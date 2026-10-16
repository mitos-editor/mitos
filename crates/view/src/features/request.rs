//! Snapshot validation shared by document features.

use editor_core::Uri;
use lsp_client::LanguageServerId;

use crate::{DocumentId, Editor};

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
        doc: &crate::Document,
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
        !self.cancel.is_canceled()
            && editor.document(self.doc).is_some_and(|doc| {
                doc.version() == self.version
                    && doc.uri() == self.uri
                    && self.servers.iter().all(|&id| {
                        editor.language_server_by_id(id).is_some()
                            && doc.supports_language_server(id)
                    })
            })
    }
}
