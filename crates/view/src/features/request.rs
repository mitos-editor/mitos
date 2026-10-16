//! Snapshot validation shared by document features.

use editor_core::{Selection, Uri};
use lsp_client::LanguageServerId;

use crate::{Document, DocumentId, Editor, View, ViewId};

/// Snapshot identity retained until a queued document-feature response is applied.
/// Cancellation while waiting on the server alone does not protect queued callbacks.
pub(crate) struct DocumentRequest {
    pub(crate) cancel: event::TaskHandle,
    doc: DocumentId,
    version: i32,
    uri: Option<Uri>,
    servers: Vec<LanguageServerId>,
}

impl DocumentRequest {
    pub(crate) fn new(
        doc: &Document,
        cancel: event::TaskHandle,
        servers: Vec<LanguageServerId>,
    ) -> Self {
        Self {
            cancel,
            doc: doc.id(),
            version: doc.version(),
            uri: doc.uri(),
            servers,
        }
    }

    pub(crate) fn is_current(&self, editor: &Editor) -> bool {
        self.get(editor).is_some()
    }

    fn get<'a>(&self, editor: &'a Editor) -> Option<&'a Document> {
        if self.cancel.is_canceled() {
            return None;
        }
        let doc = editor.document(self.doc)?;
        (doc.version() == self.version
            && doc.uri() == self.uri
            && self.servers.iter().all(|&id| {
                editor.language_server_by_id(id).is_some() && doc.supports_language_server(id)
            }))
        .then_some(doc)
    }
}

/// A document snapshot bound to one view, including views that are not focused.
pub(crate) struct ViewRequest {
    document: DocumentRequest,
    view: ViewId,
}

impl ViewRequest {
    pub(crate) fn new(
        doc: &Document,
        view: ViewId,
        cancel: event::TaskHandle,
        servers: Vec<LanguageServerId>,
    ) -> Self {
        Self {
            document: DocumentRequest::new(doc, cancel, servers),
            view,
        }
    }

    pub(crate) fn document_id(&self) -> DocumentId {
        self.document.doc
    }

    pub(crate) fn view_id(&self) -> ViewId {
        self.view
    }

    pub(crate) fn cancel_handle(&self) -> &event::TaskHandle {
        &self.document.cancel
    }

    /// Return the bound view and document while the snapshot is current.
    /// Focus, mode, and feature-specific configuration remain the caller's policy.
    pub(crate) fn get<'a>(&self, editor: &'a Editor) -> Option<(&'a View, &'a Document)> {
        let doc = self.document.get(editor)?;
        let view = editor.tree.try_get(self.view)?;
        (view.doc == doc.id()).then_some((view, doc))
    }
}

/// A view-bound snapshot that also requires the original selection.
/// Completion sessions that map edits through savepoints have a different lifetime.
pub(crate) struct SelectionRequest {
    target: ViewRequest,
    selection: Selection,
}

impl SelectionRequest {
    pub(crate) fn new(
        doc: &Document,
        view: ViewId,
        cancel: event::TaskHandle,
        servers: Vec<LanguageServerId>,
    ) -> Self {
        Self {
            target: ViewRequest::new(doc, view, cancel, servers),
            selection: doc.selection(view).clone(),
        }
    }

    pub(crate) fn document_id(&self) -> DocumentId {
        self.target.document_id()
    }

    pub(crate) fn view_id(&self) -> ViewId {
        self.target.view_id()
    }

    pub(crate) fn cancel_handle(&self) -> &event::TaskHandle {
        self.target.cancel_handle()
    }

    pub(crate) fn get<'a>(&self, editor: &'a Editor) -> Option<(&'a View, &'a Document)> {
        let (view, doc) = self.target.get(editor)?;
        (doc.selections().get(&view.id) == Some(&self.selection)).then_some((view, doc))
    }
}
