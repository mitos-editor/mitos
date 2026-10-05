//! Background Git blame, delivered only to the editor and document that requested it.

use event::{register_hook, send_blocking};
use tokio::sync::mpsc::Sender;

use super::document_debounce::debounce_documents;

use crate::{
    callbacks::EditorCallbackSender,
    document::LineBlameError,
    events::{ConfigDidChange, DocumentDidOpen},
    Document, DocumentId, Editor,
};

#[derive(Clone)]
pub struct BlameHandler {
    callbacks: EditorCallbackSender,
    refreshes: Sender<DocumentId>,
}

impl BlameHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let refreshes = debounce_documents(callbacks.clone(), request_file_blame);
        Self {
            callbacks,
            refreshes,
        }
    }

    /// Coalesce repository refreshes; explicit requests and initial loads stay immediate.
    pub(crate) fn schedule_refresh(&self, doc: DocumentId) {
        send_blocking(&self.refreshes, doc);
    }

    /// Request committed blame and optionally display a line when it arrives.
    pub(crate) fn request(&self, doc: &mut Document, trust_full: bool, line: Option<u32>) {
        let Some(request) = doc.blame_request(trust_full, line) else {
            return;
        };
        let doc_id = doc.id();
        let callbacks = self.callbacks.clone();
        tokio::spawn(async move {
            let Some(result) = request.compute().await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    let Some(doc) = editor.document_mut(doc_id) else {
                        return;
                    };
                    if let Some(line) = request.complete(doc, result) {
                        editor.show_line_blame(doc_id, line);
                    }
                })
                .await;
        });
    }
}

impl Editor {
    /// Show blame for a document line, computing the file's blame when needed.
    pub fn blame_line(&mut self, doc_id: DocumentId, line: u32) {
        let handler = self.handlers.blame.clone();
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        if doc.is_binary() || doc.path().is_none() {
            self.set_error(|| "Git blame requires a text file with a path");
            return;
        }
        if doc.file_blame().is_none() {
            let trust_full = self
                .workspace_trust
                .query(
                    doc.workspace_root(),
                    loader::workspace_trust::TrustQuery::Git,
                )
                .is_trusted();
            handler.request(doc, trust_full, Some(line));
            self.set_status("Requested blame for this file...");
        } else {
            self.show_line_blame(doc_id, line);
        }
    }

    fn show_line_blame(&mut self, doc_id: DocumentId, line: u32) {
        let config = self.config();
        let Some(doc) = self.document(doc_id) else {
            return;
        };
        match doc.line_blame(line, &config.inline_blame.format) {
            Ok(blame) if !blame.is_empty() => self.set_status(blame),
            Ok(_) | Err(LineBlameError::NotCommittedYet) => self.set_status("Not committed yet"),
            Err(LineBlameError::NotReadyYet) => {
                self.set_status(
                    "The blame for this file is not ready yet. Try again in a few seconds",
                );
            }
            Err(err @ LineBlameError::NoFileBlame(_, _)) => {
                let message = err.to_string();
                self.set_error(|| message);
            }
        }
    }
}

fn request_file_blame(editor: &mut Editor, doc_id: DocumentId) {
    // Queued automatic work may outlive an auto-fetch configuration change.
    if !editor.config().inline_blame.auto_fetch {
        return;
    }
    let handler = editor.handlers.blame.clone();
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    let trust_full = editor
        .workspace_trust
        .query(
            doc.workspace_root(),
            loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted();
    handler.request(doc, trust_full, None);
}

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        if event.editor.config().inline_blame.auto_fetch {
            request_file_blame(event.editor, event.doc);
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        // Trust changes also invalidate results fetched under the old policy.
        let trust_changed = event.old.workspace_trust != event.new.workspace_trust;
        if trust_changed {
            for doc in event.editor.documents_mut() {
                doc.invalidate_blame();
            }
        }
        if event.new.inline_blame.auto_fetch
            && (!event.old.inline_blame.auto_fetch || trust_changed)
        {
            let docs: Vec<_> = event.editor.documents().map(Document::id).collect();
            for doc in docs {
                request_file_blame(event.editor, doc);
            }
        }
        Ok(())
    });
}
