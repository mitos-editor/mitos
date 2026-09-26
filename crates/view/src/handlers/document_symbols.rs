//! Shared coordination for LSP document symbols results.

use editor_core::syntax::config::LanguageServerFeature;
use event::{cancelable_future, register_hook};
use lsp_client::lsp::DocumentSymbolResponse;

use super::lsp::DocumentRequest;
use crate::{
    callbacks::EditorCallbackSender,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidOpen, LanguageServerExited,
        LanguageServerInitialized, SelectionDidChange,
    },
    DocumentId, Editor,
};

/// Completion destination for this editor's symbols requests.
#[derive(Clone)]
pub struct DocumentSymbolsHandler {
    callbacks: EditorCallbackSender,
}

impl DocumentSymbolsHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self { callbacks }
    }
}

fn request_document_symbols(editor: &mut Editor, doc_id: DocumentId) {
    let callbacks = editor.handlers.document_symbols.callbacks.clone();
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    if !doc.breadcrumb_enabled() {
        return;
    }

    let Some(language_server) = doc
        // Get the first LSP Server that supports `DocumentSymbols`.
        .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
        .next()
    else {
        return;
    };

    let server_id = language_server.id();
    let offset_encoding = language_server.offset_encoding();
    let Some(future) = language_server.document_symbols(doc.identifier()) else {
        return;
    };
    let cancel = doc.document_symbols_controller.restart();

    let request = DocumentRequest::new(doc, cancel, vec![server_id]);

    tokio::spawn(async move {
        let Some(Ok(Some(response))) = cancelable_future(future, &request.cancel).await else {
            return;
        };

        callbacks
            .send(move |editor| {
                if !request.is_current(editor) {
                    return;
                }
                if let Some(doc) = editor.document_mut(doc_id) {
                    match response {
                        DocumentSymbolResponse::Nested(symbols) => {
                            doc.set_document_symbols(symbols, offset_encoding);
                        }
                        // TODO: Using the `Location`, it should be possible to map cursor
                        // to a hierarchical tree?
                        DocumentSymbolResponse::Flat(_) => doc.clear_document_symbols(),
                    }
                }
            })
            .await;
    });
}

pub fn register_hooks() {
    event::runtime_local! {
        static REGISTER: std::sync::Once = std::sync::Once::new();
    }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut DocumentDidOpen<'_>| {
            let doc_id = event.doc;
            let view_id = event.editor.tree.focus;
            request_document_symbols(event.editor, doc_id);
            if let Some(doc) = event.editor.document_mut(doc_id) {
                doc.update_breadcrumbs_for_view(view_id);
            }
            Ok(())
        });

        register_hook!(move |event: &mut DocumentDidChange<'_>| {
            if !event.ghost_transaction {
                // Cancel the ongoing request, if present.
                event.doc.document_symbols_controller.cancel();
                let view_id = event.view;
                let doc_id = event.doc.id();
                // PERF: Enabled breadcrumbs request fresh LSP symbols after every real edit for live
                // feedback. If insert-mode latency regresses, debounce this request path.
                if let Some(handler) = &event.doc.document_symbols_handler {
                    handler.callbacks.send_blocking(move |editor| {
                        request_document_symbols(editor, doc_id);
                        if let Some(doc) = editor.document_mut(doc_id) {
                            doc.update_breadcrumbs_for_view(view_id);
                        }
                    });
                }
            }
            Ok(())
        });

        register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
            let view_id = event.editor.tree.focus;
            if let Some(view) = event.editor.tree.try_get(view_id) {
                let doc_id = view.doc;
                request_document_symbols(event.editor, doc_id);
                if let Some(doc) = event.editor.document_mut(doc_id) {
                    doc.update_breadcrumbs_for_view(view_id);
                }
            }
            Ok(())
        });

        register_hook!(move |event: &mut LanguageServerExited<'_>| {
            for doc in event.editor.documents_mut() {
                if doc.supports_language_server(event.server_id) {
                    doc.document_symbols_controller.cancel();
                    doc.clear_document_symbols();
                }
            }
            Ok(())
        });

        register_hook!(move |event: &mut ConfigDidChange<'_>| {
            if !event.old.breadcrumb.enable && event.new.breadcrumb.enable {
                let view_id = event.editor.tree.focus;
                if let Some(view) = event.editor.tree.try_get(view_id) {
                    let doc_id = view.doc;
                    request_document_symbols(event.editor, doc_id);
                    if let Some(doc) = event.editor.document_mut(doc_id) {
                        doc.update_breadcrumbs_for_view(view_id);
                    }
                }
                return Ok(());
            }

            if event.old.breadcrumb.enable && !event.new.breadcrumb.enable {
                for doc in event.editor.documents_mut() {
                    doc.document_symbols_controller.cancel();
                    doc.clear_document_symbols();
                }
            }

            Ok(())
        });

        register_hook!(move |event: &mut SelectionDidChange<'_>| {
            event.doc.update_breadcrumbs_for_view_inlined(event.view);
            Ok(())
        });
    });
}
