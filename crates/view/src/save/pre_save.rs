//! Bounded LSP save hooks. A slow, failed, or stale response never prevents writing.

use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    time::Duration,
};

use editor_core::Transaction;
use lsp_client::{lsp, LanguageServerId};
use tokio::time::Instant;

use crate::{features::formatting::FormatSnapshot, Document, DocumentId, Editor};

pub(super) struct PreSave {
    servers: VecDeque<LanguageServerId>,
    deadline: Instant,
}

pub(super) struct Update {
    snapshot: FormatSnapshot,
    result: anyhow::Result<Option<Transaction>>,
}

impl PreSave {
    pub(super) fn new(
        editor: &Editor,
        doc: &Document,
        reason: lsp::TextDocumentSaveReason,
    ) -> Self {
        let servers = if editor.config().lsp.enable && !doc.is_content_readonly() {
            let mut seen = HashSet::new();
            doc.language_servers()
                .filter(|server| seen.insert(server.id()))
                .map(|server| {
                    server.text_document_will_save(lsp::WillSaveTextDocumentParams {
                        text_document: doc.identifier(),
                        reason,
                    });
                    server.id()
                })
                .collect()
        } else {
            VecDeque::new()
        };
        Self {
            servers,
            deadline: Instant::now() + Duration::from_millis(1500),
        }
    }

    /// Providers run in order against each predecessor's edits, sharing one save budget.
    pub(super) fn next(
        &mut self,
        editor: &Editor,
        doc: &Document,
        reason: lsp::TextDocumentSaveReason,
    ) -> Option<impl Future<Output = Update> + use<>> {
        if !editor.config().lsp.enable {
            return None;
        }
        while let Some(server) = self.servers.pop_front() {
            if Instant::now() >= self.deadline {
                break;
            }
            let Some(client) = editor
                .language_server_by_id(server)
                .filter(|_| doc.supports_language_server(server))
            else {
                continue;
            };
            let Some(request) =
                client.text_document_will_save_wait_until(lsp::WillSaveTextDocumentParams {
                    text_document: doc.identifier(),
                    reason,
                })
            else {
                continue;
            };
            let deadline = self.deadline;
            let encoding = client.offset_encoding();
            let snapshot = FormatSnapshot::new(editor, doc, Some(server));
            let text = doc.text().clone();
            return Some(async move {
                let result = async {
                    let edits = tokio::time::timeout_at(deadline, request).await??;
                    edits
                        .map(|edits| {
                            crate::language_servers::edits::generate_transaction_from_edits(
                                &text, edits, encoding,
                            )
                        })
                        .transpose()
                        .map_err(Into::into)
                }
                .await;
                Update { snapshot, result }
            });
        }
        None
    }
}

impl Update {
    pub(super) fn apply(self, editor: &mut Editor, document: DocumentId) {
        let Some(doc) = editor.document(document) else {
            return;
        };
        if !editor.config().lsp.enable || !self.snapshot.is_current(editor, doc) {
            return;
        }
        match self.result {
            Ok(Some(transaction)) => {
                let version = doc.version();
                if let Err(error) =
                    editor.apply_document_changes(document, version, transaction.changes())
                {
                    log::info!("failed to apply pre-save edits: {error}");
                }
            }
            Ok(None) => (),
            Err(error) => log::info!("pre-save edits skipped: {error}"),
        }
    }
}
