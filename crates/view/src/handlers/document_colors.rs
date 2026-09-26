//! Shared coordination for LSP document colors results.

use std::{collections::HashSet, time::Duration};

use editor_core::{syntax::config::LanguageServerFeature, text_annotations::InlineAnnotation};
use event::{cancelable_future, register_hook};
use futures_util::{stream::FuturesUnordered, StreamExt};
use lsp_client::lsp;
use tokio::time::Instant;

use super::lsp::DocumentRequest;
use crate::{
    callbacks::EditorCallbackSender,
    document::DocumentColorSwatches,
    events::{DocumentDidChange, DocumentDidOpen, LanguageServerExited, LanguageServerInitialized},
    DocumentId, Editor, Theme,
};

struct DocumentColorsEvent(DocumentId);

/// Scheduling handle attached to documents owned by this editor.
#[derive(Clone)]
pub struct DocumentColorsHandler {
    callbacks: EditorCallbackSender,
    events: tokio::sync::mpsc::Sender<DocumentColorsEvent>,
}

impl DocumentColorsHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        use event::AsyncHook as _;
        let events = Debounce {
            callbacks: callbacks.clone(),
            docs: HashSet::new(),
        }
        .spawn();
        Self { callbacks, events }
    }
}

struct Debounce {
    callbacks: EditorCallbackSender,
    docs: HashSet<DocumentId>,
}

const DOCUMENT_CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

impl event::AsyncHook for Debounce {
    type Event = DocumentColorsEvent;

    fn handle_event(&mut self, event: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let DocumentColorsEvent(doc_id) = event;
        self.docs.insert(doc_id);
        Some(Instant::now() + DOCUMENT_CHANGE_DEBOUNCE)
    }

    fn finish_debounce(&mut self) {
        let docs = std::mem::take(&mut self.docs);

        self.callbacks.send_blocking(move |editor| {
            for doc in docs {
                request_document_colors(editor, doc, None);
            }
        });
    }
}

fn request_document_colors(
    editor: &mut Editor,
    doc_id: DocumentId,
    exited_server: Option<lsp_client::LanguageServerId>,
) {
    let callbacks = editor.handlers.document_colors.callbacks.clone();
    if !editor.config().lsp.display_color_swatches {
        return;
    }

    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    let cancel = doc.color_swatch_controller.restart();

    // Exit hooks run before the server is removed from the registry.
    let mut seen_language_servers = HashSet::new();
    let mut futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::DocumentColors)
        .filter(|ls| Some(ls.id()) != exited_server)
        .filter(|ls| seen_language_servers.insert(ls.id()))
        .map(|language_server| {
            let text = doc.text().clone();
            let offset_encoding = language_server.offset_encoding();
            let future = language_server
                .text_document_document_color(doc.identifier(), None)
                .unwrap();

            async move {
                let colors: Vec<_> = future
                    .await?
                    .into_iter()
                    .filter_map(|color_info| {
                        let pos = lsp_client::util::lsp_pos_to_pos(
                            &text,
                            color_info.range.start,
                            offset_encoding,
                        )?;
                        Some((pos, color_info.color))
                    })
                    .collect();
                anyhow::Ok(colors)
            }
        })
        .collect();

    if futures.is_empty() {
        return;
    }

    let request = DocumentRequest::new(doc, cancel, seen_language_servers.into_iter().collect());

    tokio::spawn(async move {
        let mut all_colors = Vec::new();
        loop {
            match cancelable_future(futures.next(), &request.cancel).await {
                Some(Some(Ok(items))) => all_colors.extend(items),
                Some(Some(Err(err))) => log::error!("document color request failed: {err}"),
                Some(None) => break,
                // The request was cancelled.
                None => return,
            }
        }
        callbacks
            .send(move |editor| {
                if request.is_current(editor) {
                    attach_document_colors(editor, doc_id, all_colors);
                }
            })
            .await;
    });
}

fn attach_document_colors(
    editor: &mut Editor,
    doc_id: DocumentId,
    mut doc_colors: Vec<(usize, lsp::Color)>,
) {
    if !editor.config().lsp.display_color_swatches {
        return;
    }

    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };

    if doc_colors.is_empty() {
        doc.color_swatches.take();
        return;
    }

    doc_colors.sort_by_key(|(pos, _)| *pos);

    let mut color_swatches = Vec::with_capacity(doc_colors.len());
    let mut color_swatches_padding = Vec::with_capacity(doc_colors.len());
    let mut colors = Vec::with_capacity(doc_colors.len());

    for (pos, color) in doc_colors {
        color_swatches_padding.push(InlineAnnotation::new(pos, " "));
        color_swatches.push(InlineAnnotation::new(pos, "■"));
        colors.push(Theme::rgb_highlight(
            (color.red * 255.) as u8,
            (color.green * 255.) as u8,
            (color.blue * 255.) as u8,
        ));
    }

    doc.color_swatches = Some(DocumentColorSwatches {
        color_swatches,
        colors,
        color_swatches_padding,
    });
}

pub fn register_hooks() {
    event::runtime_local! {
        static REGISTER: std::sync::Once = std::sync::Once::new();
    }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut DocumentDidOpen<'_>| {
            // when a document is initially opened, request colors for it
            request_document_colors(event.editor, event.doc, None);

            Ok(())
        });

        register_hook!(move |event: &mut DocumentDidChange<'_>| {
            // Update the color swatch' positions, helping ensure they are displayed in the
            // proper place.
            let apply_color_swatch_changes = |annotations: &mut Vec<InlineAnnotation>| {
                event.changes.update_positions(
                    annotations
                        .iter_mut()
                        .map(|annotation| (&mut annotation.char_idx, editor_core::Assoc::After)),
                );
            };

            if let Some(DocumentColorSwatches {
                color_swatches,
                colors: _colors,
                color_swatches_padding,
            }) = &mut event.doc.color_swatches
            {
                apply_color_swatch_changes(color_swatches);
                apply_color_swatch_changes(color_swatches_padding);
            }

            // Avoid re-requesting document colors if the change is a ghost transaction (completion)
            // because the language server will not know about the updates to the document and will
            // give out-of-date locations.
            if !event.ghost_transaction {
                // Cancel the ongoing request, if present.
                event.doc.color_swatch_controller.cancel();
                if let Some(handler) = &event.doc.document_colors_handler {
                    event::send_blocking(&handler.events, DocumentColorsEvent(event.doc.id()));
                }
            }

            Ok(())
        });

        register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
            let doc_ids: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();

            for doc_id in doc_ids {
                request_document_colors(event.editor, doc_id, None);
            }

            Ok(())
        });

        register_hook!(move |event: &mut LanguageServerExited<'_>| {
            // Clear and re-request all color swatches when a server exits.
            for doc in event.editor.documents_mut() {
                if doc.supports_language_server(event.server_id) {
                    doc.color_swatches.take();
                }
            }

            let doc_ids: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();

            for doc_id in doc_ids {
                request_document_colors(event.editor, doc_id, Some(event.server_id));
            }

            Ok(())
        });
    });
}
