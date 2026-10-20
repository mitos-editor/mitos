//! Snapshot all open documents, then validate only the operation's affected paths.

use std::{collections::HashMap, path::PathBuf};

use anyhow::{ensure, Result};
use editor_core::Uri;
use lsp_client::lsp;

use super::FileOperation;
use crate::{Document, DocumentId, Editor};

struct DocumentState {
    buffer: buffer::Snapshot,
    uri: Option<Uri>,
}

impl DocumentState {
    fn matches(&self, doc: &Document) -> bool {
        self.buffer.is_current(&doc.buffer) && self.uri == doc.uri()
    }
}

pub(super) struct DocumentGuard {
    documents: HashMap<DocumentId, DocumentState>,
}

impl DocumentGuard {
    pub(super) fn capture(editor: &Editor) -> Self {
        // Returned edits may target any document, including with no version.
        // Each provider gets a fresh snapshot after the previous provider's edits.
        Self {
            documents: editor
                .documents()
                .map(|doc| {
                    (
                        doc.id(),
                        DocumentState {
                            buffer: doc.snapshot(),
                            uri: doc.uri(),
                        },
                    )
                })
                .collect(),
        }
    }

    pub(super) fn validate(
        &self,
        editor: &Editor,
        operation: &FileOperation,
        edit: Option<&lsp::WorkspaceEdit>,
    ) -> Result<()> {
        let mut paths: Vec<_> = operation.paths().into_iter().map(PathBuf::from).collect();
        if let Some(edit) = edit {
            append_edit_paths(&mut paths, edit)?;
        }
        let affected = |uri: Option<&Uri>| {
            uri.and_then(Uri::as_path)
                .is_some_and(|path| paths.iter().any(|root| path.starts_with(root)))
        };
        // Check both snapshots and live documents: closing or renaming an affected
        // buffer must fail, as must opening a new buffer at an affected path.
        ensure!(
            self.documents
                .iter()
                .filter(|(_, state)| affected(state.uri.as_ref()))
                .all(|(&id, state)| editor.document(id).is_some_and(|doc| state.matches(doc)))
                && editor
                    .documents()
                    .filter(|doc| affected(doc.uri().as_ref()))
                    .all(|doc| self
                        .documents
                        .get(&doc.id())
                        .is_some_and(|state| state.matches(doc))),
            "Documents changed while preparing file operation; try again"
        );
        Ok(())
    }
}

fn append_edit_paths(paths: &mut Vec<PathBuf>, edit: &lsp::WorkspaceEdit) -> Result<()> {
    let mut append = |url: &lsp_client::Url| -> Result<()> {
        let uri = Uri::try_from(url)?;
        if let Some(path) = uri.as_path() {
            paths.push(path.to_owned());
        }
        Ok(())
    };
    // Follow the same precedence as apply_workspace_edit.
    match &edit.document_changes {
        Some(lsp::DocumentChanges::Edits(edits)) => {
            for edit in edits {
                append(&edit.text_document.uri)?;
            }
        }
        Some(lsp::DocumentChanges::Operations(operations)) => {
            for operation in operations {
                match operation {
                    lsp::DocumentChangeOperation::Edit(edit) => append(&edit.text_document.uri)?,
                    lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(op)) => {
                        append(&op.uri)?
                    }
                    lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Delete(op)) => {
                        append(&op.uri)?
                    }
                    lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(op)) => {
                        append(&op.old_uri)?;
                        append(&op.new_uri)?;
                    }
                }
            }
        }
        None => {
            if let Some(changes) = &edit.changes {
                for uri in changes.keys() {
                    append(uri)?;
                }
            }
        }
    }
    Ok(())
}
