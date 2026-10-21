//! One automatic refresh queue and lifecycle policy for document-wide LSP caches.

use super::{code_lenses, debounce::debounce_documents, document_colors, document_links};
use crate::events::{
    ConfigDidChange, DocumentDidChange, DocumentDidOpen, LanguageServerExited,
    LanguageServerInitialized,
};
use crate::{callbacks::EditorCallbackSender, DocumentId, Editor};
use event::register_hook;
use lsp_client::LanguageServerId;
use tokio::sync::mpsc::Sender;

/// Services installed before documents join an editor. Interactive operations keep
/// their own cancellation; only automatic cache refreshes share this scheduler.
#[derive(Clone)]
pub struct DocumentRefreshHandler {
    pub(crate) callbacks: EditorCallbackSender,
    events: Sender<DocumentId>,
}

impl DocumentRefreshHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            events: debounce_documents(callbacks.clone(), |editor, doc| request(editor, doc, None)),
            callbacks,
        }
    }
}

fn request(editor: &mut Editor, doc: DocumentId, exited: Option<LanguageServerId>) {
    if !editor.config().lsp.enable {
        return;
    }
    document_colors::request_document_colors(editor, doc, exited);
    document_links::request_document_links(editor, doc, exited);
    code_lenses::request_document_code_lenses(editor, doc, exited);
}

/// Refresh after language, path, or provider changes, including reused servers.
pub(crate) fn refresh_document(editor: &mut Editor, doc: DocumentId) {
    if let Some(doc) = editor.document_mut(doc) {
        doc.document_colors.invalidate();
        doc.document_links.invalidate();
        doc.code_lenses.clear();
    }
    request(editor, doc, None);
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request(event.editor, event.doc, None);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event.doc.document_colors.cancel_refresh();
        event.doc.document_links.cancel_refresh();
        if let Some(handler) = &event.doc.document_refresh {
            event::send_blocking(&handler.events, event.doc.id());
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let docs: Vec<_> = event
            .editor
            .documents()
            .filter(|doc| doc.supports_language_server(event.server_id))
            .map(|doc| doc.id())
            .collect();
        for doc in docs {
            request(event.editor, doc, None);
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        let docs: Vec<_> = event
            .editor
            .documents_mut()
            .filter(|doc| doc.supports_language_server(event.server_id))
            .map(|doc| {
                doc.document_colors.clear_cache();
                doc.document_links.clear_cache();
                doc.code_lenses.remove_server(event.server_id);
                doc.id()
            })
            .collect();
        for doc in docs {
            request(event.editor, doc, Some(event.server_id));
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        // Lens visibility and resolution policy depend on editor configuration.
        let docs: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();
        for doc in docs {
            code_lenses::refresh_document(event.editor, doc);
        }
        Ok(())
    });
}
