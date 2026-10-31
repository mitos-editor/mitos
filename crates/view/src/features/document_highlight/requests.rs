//! Shared coordination for LSP document highlights results.

use event::register_hook;
use language_config::LanguageServerFeature;
use lsp_client::{lsp, util::lsp_range_to_range, OffsetEncoding};

use crate::{
    callbacks::EditorCallbackSender,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidOpen, DocumentLanguageDidChange,
        LanguageServerExited, LanguageServerInitialized, SelectionDidChange,
    },
    features::request::DocumentRequest,
    DocumentId, Editor, ViewId,
};

/// Completion destination for this editor's highlight requests.
#[derive(Clone)]
pub struct DocumentHighlightHandler {
    callbacks: EditorCallbackSender,
}

impl DocumentHighlightHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self { callbacks }
    }
}

fn request_document_highlights(editor: &mut Editor, doc_id: DocumentId, view_id: ViewId) {
    let callbacks = editor.handlers.document_highlight.callbacks.clone();
    if !editor.config().lsp.document_highlight.auto_trigger {
        return;
    }

    if !editor
        .tree
        .try_get(view_id)
        .is_some_and(|view| view.doc == doc_id)
    {
        return;
    }

    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    if !doc.has_language_server_with_feature(LanguageServerFeature::DocumentHighlight) {
        doc.clear_document_highlights(view_id);
        return;
    }

    doc.ensure_view_init(view_id);
    let Some((server_id, offset_encoding, future)) = (match doc
        .language_servers_with_feature(LanguageServerFeature::DocumentHighlight)
        .next()
    {
        Some(language_server) => {
            let offset_encoding = language_server.offset_encoding();
            let pos = doc.position(view_id, offset_encoding);
            language_server
                .text_document_document_highlight(doc.identifier(), pos, None)
                .map(|future| (language_server.id(), offset_encoding, future))
        }
        None => None,
    }) else {
        doc.clear_document_highlights(view_id);
        return;
    };

    let text = doc.text().clone();
    let cancel = doc.document_highlight_controller(view_id).restart();

    let selection = doc.selection(view_id).clone();
    let request = DocumentRequest::new(doc, cancel, vec![server_id]);

    request.publish(
        callbacks,
        async move {
            match future.await {
                Ok(response) => Some(
                    response
                        .map(|highlights| {
                            document_highlight_ranges(&text, offset_encoding, highlights)
                        })
                        .unwrap_or_default(),
                ),
                Err(err) => {
                    log::error!("document highlight request failed: {err}");
                    None
                }
            }
        },
        move |editor, ranges| {
            if editor
                .tree
                .try_get(view_id)
                .is_some_and(|view| view.doc == doc_id)
                && editor
                    .document(doc_id)
                    .is_some_and(|doc| doc.selection(view_id) == &selection)
            {
                apply_document_highlights(editor, doc_id, view_id, ranges);
            }
        },
    );
}

fn document_highlight_ranges(
    text: &editor_core::Rope,
    offset_encoding: OffsetEncoding,
    highlights: Vec<lsp::DocumentHighlight>,
) -> Vec<std::ops::Range<usize>> {
    let slice = text.slice(..);
    let mut ranges: Vec<_> = highlights
        .into_iter()
        .filter_map(|highlight| lsp_range_to_range(text, highlight.range, offset_encoding))
        .map(|range| range.min_width_1(slice))
        .filter_map(|range| {
            let start = range.from();
            let end = range.to();
            (start < end).then_some(start..end)
        })
        .collect();

    ranges.sort_by_key(|a| (a.start, a.end));

    let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            if range.end > last.end {
                last.end = range.end;
            }
            continue;
        }
        merged.push(range);
    }

    merged
}

fn apply_document_highlights(
    editor: &mut Editor,
    doc_id: DocumentId,
    view_id: ViewId,
    ranges: Vec<std::ops::Range<usize>>,
) {
    if !editor.config().lsp.document_highlight.auto_trigger {
        return;
    }

    if !editor
        .tree
        .try_get(view_id)
        .is_some_and(|view| view.doc == doc_id)
    {
        return;
    }

    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    if !doc.has_language_server_with_feature(LanguageServerFeature::DocumentHighlight) {
        doc.clear_document_highlights(view_id);
        return;
    }

    if ranges.is_empty() {
        doc.clear_document_highlights(view_id);
        return;
    }

    doc.set_document_highlights(view_id, ranges);
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentLanguageDidChange<'_>| {
        let view_id = event.editor.tree.focus();
        request_document_highlights(event.editor, event.doc, view_id);
        Ok(())
    });

    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if event.doc.config.load().lsp.document_highlight.auto_trigger
            && event
                .doc
                .has_language_server_with_feature(LanguageServerFeature::DocumentHighlight)
        {
            let doc_id = event.doc.id();
            let view_id = event.view;
            event.doc.document_highlight_controller(view_id).cancel();
            if let Some(handler) = &event.doc.document_highlights.handler {
                handler.callbacks.send_blocking(move |editor| {
                    request_document_highlights(editor, doc_id, view_id);
                });
            }
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        if !event.editor.config().lsp.document_highlight.auto_trigger {
            return Ok(());
        }
        let view_id = event.editor.tree.focus();
        if event.editor.tree.try_get(view_id).is_none() {
            return Ok(());
        }
        request_document_highlights(event.editor, event.doc, view_id);
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if event.doc.config.load().lsp.document_highlight.auto_trigger
            && let Some(view_id) = event.view
            && event
                .doc
                .has_language_server_with_feature(LanguageServerFeature::DocumentHighlight)
        {
            let doc_id = event.doc.id();
            event.doc.document_highlight_controller(view_id).cancel();
            if let Some(handler) = &event.doc.document_highlights.handler {
                handler.callbacks.send_blocking(move |editor| {
                    request_document_highlights(editor, doc_id, view_id);
                });
            }
        }
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        if !event.editor.config().lsp.document_highlight.auto_trigger {
            return Ok(());
        }
        let view_id = event.editor.tree.focus();
        let Some(view) = event.editor.tree.try_get(view_id) else {
            return Ok(());
        };
        let doc_id = view.doc;
        request_document_highlights(event.editor, doc_id, view_id);
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.clear_all_document_highlights();
            }
        }
        Ok(())
    });

    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        // When auto document highlight is turned on, request highlights immediately
        // for the focused view instead of waiting for the next selection change.
        if !event.old.lsp.document_highlight.auto_trigger
            && event.new.lsp.document_highlight.auto_trigger
        {
            let view_id = event.editor.tree.focus();
            let Some(view) = event.editor.tree.try_get(view_id) else {
                return Ok(());
            };

            request_document_highlights(event.editor, view.doc, view_id);
            return Ok(());
        }

        // When auto document highlight is turned off, clear any highlights that were
        // previously rendered across open documents.
        if event.old.lsp.document_highlight.auto_trigger
            && !event.new.lsp.document_highlight.auto_trigger
        {
            for doc in event.editor.documents_mut() {
                doc.clear_all_document_highlights();
            }
        }
        Ok(())
    });
}
