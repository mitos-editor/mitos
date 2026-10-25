//! Shared coordination for LSP document colors results.

use event::register_hook;
use language_config::LanguageServerFeature;
use lsp_client::lsp;
use text_layout::text_annotations::InlineAnnotation;

use crate::{
    callbacks::EditorCallbackSender,
    events::ConfigDidChange,
    features::{
        debounce::debounce_documents,
        request::{collect_document_results, document_providers, register_document_refresh_hooks},
    },
    DocumentId, Editor,
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
            request_document_colors(editor, doc, None);
        });
        Self { callbacks, events }
    }
}

fn request_document_colors(
    editor: &mut Editor,
    doc_id: DocumentId,
    exited_server: Option<lsp_client::LanguageServerId>,
) {
    let callbacks = editor.handlers.document_colors.callbacks.clone();
    if !editor.config().lsp.enable || !editor.config().lsp.color_swatches.enable {
        return;
    }

    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    let cancel = doc.document_colors.request.restart();

    let requests = document_providers(doc, LanguageServerFeature::DocumentColors, exited_server)
        .map(|language_server| {
            let text = doc.text().clone();
            let offset_encoding = language_server.offset_encoding();
            let future = language_server
                .text_document_document_color(doc.identifier(), None)
                .unwrap();

            (language_server.id(), async move {
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
            })
        });
    collect_document_results(
        doc,
        cancel,
        callbacks,
        requests,
        "document color",
        attach_document_colors,
    );
}

fn attach_document_colors(
    editor: &mut Editor,
    doc_id: DocumentId,
    mut doc_colors: Vec<(usize, lsp::Color)>,
) {
    if !editor.config().lsp.enable || !editor.config().lsp.color_swatches.enable {
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

pub(crate) fn register_hooks() {
    register_document_refresh_hooks(
        request_document_colors,
        |doc| {
            doc.document_colors.request.cancel();
            if let Some(handler) = &doc.document_colors.handler {
                event::send_blocking(&handler.events, doc.id());
            }
        },
        |doc| doc.document_colors.clear_cache(),
    );
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if event.old.lsp.enable != event.new.lsp.enable
            || event.old.lsp.color_swatches.enable != event.new.lsp.color_swatches.enable
        {
            let docs: Vec<_> = event
                .editor
                .documents_mut()
                .map(|doc| {
                    doc.document_colors.invalidate();
                    doc.id()
                })
                .collect();
            for doc in docs {
                request_document_colors(event.editor, doc, None);
            }
        }
        Ok(())
    });
}
