//! Shared coordination for LSP document colors results.

use futures_util::stream::FuturesUnordered;
use language_config::LanguageServerFeature;
use lsp_client::lsp;
use text_layout::text_annotations::InlineAnnotation;

use crate::{
    callbacks::EditorCallbackSender,
    features::{
        debounce::debounce_documents,
        request::{collect_responses, DocumentRequest},
    },
    Document, DocumentId, Editor,
};
use theme::Theme;

use super::DocumentColorSwatches;

/// Scheduling handle attached to documents owned by this editor.
#[derive(Clone)]
pub struct DocumentColorsHandler {
    callbacks: EditorCallbackSender,
    events: tokio::sync::mpsc::Sender<DocumentId>,
}

impl DocumentColorsHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let events = debounce_documents(callbacks.clone(), |editor, doc| {
            refresh(editor, doc, false);
        });
        Self { callbacks, events }
    }

    pub(in crate::features) fn refresh_after_edit(doc: &mut Document, handler: Option<&Self>) {
        doc.document_colors.request.cancel();
        if let Some(handler) = handler {
            event::send_blocking(&handler.events, doc.id());
        }
    }
}

pub(in crate::features) fn refresh(editor: &mut Editor, doc_id: DocumentId, server_exited: bool) {
    let callbacks = editor.handlers.document_colors.callbacks.clone();
    let enabled = editor.config().lsp.color_swatches.enable;
    let registry = &editor.language_servers;
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    if server_exited {
        doc.document_colors.clear_cache();
    }
    if !enabled {
        return;
    }

    let cancel = doc.document_colors.request.restart();

    let mut servers = Vec::new();
    let futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::DocumentColors)
        .filter(|server| registry.is_ready(server.id()))
        .inspect(|server| servers.push(server.id()))
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

    let request = DocumentRequest::new(doc, cancel, servers);

    tokio::spawn(async move {
        let Some(all_colors) = collect_responses(futures, &request.cancel, "document color").await
        else {
            return;
        };
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
    if !editor.config().lsp.color_swatches.enable {
        return;
    }

    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };

    if doc_colors.is_empty() {
        doc.document_colors.clear_cache();
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

    doc.document_colors.cache = Some(DocumentColorSwatches {
        color_swatches,
        colors,
        color_swatches_padding,
    });
}
