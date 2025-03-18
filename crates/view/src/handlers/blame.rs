//! Background Git blame, delivered only to the editor and document that requested it.

use event::{cancelable_future, register_hook};
use vcs::FileBlame;

use crate::{
    callbacks::EditorCallbackSender,
    document::LineBlameError,
    events::{ConfigDidChange, DocumentDidOpen},
    Document, DocumentId, Editor,
};

#[derive(Clone)]
pub struct BlameHandler {
    callbacks: EditorCallbackSender,
}

impl BlameHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self { callbacks }
    }

    /// Refresh a document's committed blame. A line requests a status message on completion.
    pub(crate) fn request(&self, doc: &mut Document, trust_full: bool, line: Option<u32>) {
        if doc.is_binary() {
            return;
        }
        let Some(path) = doc.path().map(ToOwned::to_owned) else {
            return;
        };
        let doc_id = doc.id();
        let cancel = doc.blame_request.restart();
        let callbacks = self.callbacks.clone();
        let blame_path = path.clone();
        let worker =
            tokio::task::spawn_blocking(move || FileBlame::try_new(blame_path, trust_full));
        tokio::spawn(async move {
            let Some(result) = cancelable_future(worker, &cancel).await else {
                return;
            };
            let result = result.unwrap_or_else(|err| Err(err.into()));
            callbacks
                .send(move |editor| {
                    if cancel.is_canceled() {
                        return;
                    }
                    let Some(doc) = editor.document_mut(doc_id) else {
                        return;
                    };
                    if doc.path() != Some(&path) || doc.is_binary() {
                        return;
                    }
                    doc.file_blame = Some(result);
                    if let Some(line) = line {
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
        if doc.file_blame.is_none() {
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
