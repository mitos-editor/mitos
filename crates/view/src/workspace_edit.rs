//! Workspace edits shared by server requests, code actions, rename, and save actions.
//!
//! Ordered changes run in protocol order and stop at the first failure. Completed document
//! edits and filesystem operations remain applied, matching the advertised LSP
//! `abort` failure-handling capability.

pub mod confirmation;
mod snippet;

use lsp_client::UriExt as _;
use std::path::PathBuf;

use editor_core::movement::Direction;
use lsp_client::{lsp, OffsetEncoding};

use crate::editor::ExistingTarget;
use crate::{editing::EditError, editor::Action, Editor};

/// A failed workspace change and its index in the supplied change sequence.
#[derive(Debug, thiserror::Error)]
#[error("workspace change {failed_change_idx}: {kind}")]
pub struct ApplyEditError {
    #[source]
    pub kind: ApplyEditErrorKind,
    pub failed_change_idx: usize,
}

/// The original rejection from document editing, URL conversion, or filesystem access.
#[derive(Debug, thiserror::Error)]
pub enum ApplyEditErrorKind {
    #[error("unknown workspace edit annotation: {0}")]
    UnknownAnnotation(String),
    #[error("workspace edit requires confirmation")]
    ConfirmationRequired,
    #[error("unsupported snippet string kind: {0}")]
    InvalidSnippetKind(String),
    #[error("invalid workspace snippet: {0}")]
    InvalidSnippet(#[source] anyhow::Error),
    #[error(transparent)]
    InvalidTextEdit(#[from] crate::language_servers::edits::TextEditError),
    #[error(transparent)]
    Document(#[from] EditError),
    #[error("failed to open document {path}: {source}")]
    OpenDocument {
        path: PathBuf,
        source: crate::document::DocumentOpenError,
    },
    #[error(transparent)]
    InvalidUrl(#[from] lsp_client::UriError),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

impl Editor {
    /// Merge text-only actions and preflight every target before mutating any buffer.
    /// Each document receives one transaction, keeping all cursor edits in one undo step.
    pub(crate) fn apply_code_action_edits(
        &mut self,
        encoding: OffsetEncoding,
        actions: Vec<lsp::WorkspaceEdit>,
    ) -> anyhow::Result<()> {
        crate::features::linked_editing::cancel(self);
        let mut documents: Vec<(lsp_client::Url, Option<i32>, Vec<lsp::DocumentTextEdit>)> =
            Vec::new();
        for action in actions {
            confirmation::validate(&action)?;
            let incoming = match action.document_changes {
                Some(lsp::DocumentChanges::Edits(edits)) => edits,
                Some(lsp::DocumentChanges::Operations(operations)) => operations
                    .into_iter()
                    .map(|op| match op {
                        lsp::DocumentChangeOperation::Edit(edit) => Ok(edit),
                        lsp::DocumentChangeOperation::Op(_) => {
                            anyhow::bail!("File operations cannot be applied across selections")
                        }
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
                None => action
                    .changes
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(uri, edits)| lsp::TextDocumentEdit {
                        text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                            uri,
                            version: None,
                        },
                        edits: edits.into_iter().map(Into::into).collect(),
                    })
                    .collect(),
            };
            for document in incoming {
                let uri = document.text_document.uri;
                let version = document.text_document.version;
                let index = documents
                    .iter()
                    .position(|(target, ..)| *target == uri)
                    .unwrap_or_else(|| {
                        documents.push((uri, version, Vec::new()));
                        documents.len() - 1
                    });
                let (_, expected, edits) = &mut documents[index];
                anyhow::ensure!(
                    expected.is_none() || version.is_none() || *expected == version,
                    "Code actions disagree on document version"
                );
                *expected = expected.or(version);
                let previous_len = edits.len();
                for edit in document.edits {
                    // Shared imports and file-wide fixes often occur at several cursors.
                    if !edits[..previous_len]
                        .iter()
                        .any(|previous| snippet::same_edit(previous, &edit))
                    {
                        edits.push(edit);
                    }
                }
            }
        }
        let mut prepared = Vec::new();
        for (url, expected, mut edits) in documents {
            edits.sort_by_key(|edit| {
                let range = snippet::range(edit);
                (range.start, range.end)
            });
            // Distinct inserts at the same position have no unambiguous ordering
            // across actions. Equal inserts have already been deduplicated.
            anyhow::ensure!(
                !edits.windows(2).any(|pair| {
                    let left = snippet::range(&pair[0]);
                    let right = snippet::range(&pair[1]);
                    left.start == left.end && right.start == right.end && left.start == right.start
                }),
                "Code actions contain conflicting inserts"
            );
            let uri = url.to_editor_uri()?;
            let id = self.open(uri.as_path().expect("URIs are valid paths"), Action::Load)?;
            anyhow::ensure!(
                !prepared.iter().any(|(target, _, _)| *target == id),
                "Code actions contain aliases for the same document"
            );
            let doc = self.document(id).unwrap();
            anyhow::ensure!(
                expected.is_none_or(|version| version == doc.version()),
                "Code action is no longer current"
            );
            let edit = snippet::prepare(self, doc, edits, encoding)?;
            doc.validate_changes(edit.transaction.changes())?;
            prepared.push((id, doc.version(), edit));
        }
        for (id, version, edit) in prepared {
            self.apply_prepared_text_edits(id, version, edit)?;
        }
        crate::features::linked_editing::refresh(self);
        Ok(())
    }

    fn apply_text_edits(
        &mut self,
        url: &lsp_client::Url,
        version: Option<i32>,
        text_edits: Vec<lsp::DocumentTextEdit>,
        offset_encoding: OffsetEncoding,
    ) -> Result<(), ApplyEditErrorKind> {
        let uri = match url.to_editor_uri() {
            Ok(uri) => uri,
            Err(err) => {
                log::error!("{err}");
                return Err(err.into());
            }
        };
        let path = uri.as_path().expect("URIs are valid paths");

        let doc_id = match self.open(path, Action::Load) {
            Ok(doc_id) => doc_id,
            Err(err) => {
                let error = ApplyEditErrorKind::OpenDocument {
                    path: path.to_owned(),
                    source: err,
                };
                self.set_error(|| error.to_string());
                return Err(error);
            }
        };

        let actual_version = doc!(self, &doc_id).version();
        if let Some(expected) = version
            && expected != actual_version
        {
            let error = EditError::DocumentChanged {
                expected,
                actual: actual_version,
            };
            self.set_error(|| {
                let message = format!("outdated workspace edit for {path:?}: {error}");
                log::error!("{message}");
                message
            });
            return Err(error.into());
        }

        let doc = doc!(self, &doc_id);
        let edit = snippet::prepare(self, doc, text_edits, offset_encoding)?;
        self.apply_prepared_text_edits(doc_id, actual_version, edit)?;
        Ok(())
    }

    fn apply_prepared_text_edits(
        &mut self,
        id: crate::DocumentId,
        version: i32,
        edit: snippet::Prepared,
    ) -> Result<(), EditError> {
        self.apply_document_changes(id, version, edit.transaction.changes())?;
        if let Some((view, rendered)) = edit.interactive
            && self.tree.focus() == view
            && self.tree.try_get(view).is_some_and(|view| view.doc == id)
        {
            let doc = self.documents.get_mut(&id).unwrap();
            doc.clear_snippet();
            doc.set_selection(view, rendered.first_selection(Direction::Forward, 0));
            doc.install_snippet(view, rendered);
            self.set_mode(crate::document::Mode::Insert);
        }
        Ok(())
    }

    /// Apply changes in order, stopping on failure without rolling back earlier changes.
    /// A failed index refers to `document_changes`, or to iteration order in `changes`.
    pub fn apply_workspace_edit(
        &mut self,
        offset_encoding: OffsetEncoding,
        workspace_edit: &lsp::WorkspaceEdit,
    ) -> Result<(), ApplyEditError> {
        confirmation::validate(workspace_edit)?;
        crate::features::linked_editing::cancel(self);
        if let Some(ref document_changes) = workspace_edit.document_changes {
            match document_changes {
                lsp::DocumentChanges::Edits(document_edits) => {
                    for (i, document_edit) in document_edits.iter().enumerate() {
                        self.apply_text_document_edit(document_edit, offset_encoding)
                            .map_err(|kind| ApplyEditError {
                                kind,
                                failed_change_idx: i,
                            })?;
                    }
                }
                lsp::DocumentChanges::Operations(operations) => {
                    log::debug!("document changes - operations: {:?}", operations);
                    for (i, operation) in operations.iter().enumerate() {
                        let result = match operation {
                            lsp::DocumentChangeOperation::Op(op) => {
                                self.apply_document_resource_op(op)
                            }
                            lsp::DocumentChangeOperation::Edit(document_edit) => {
                                self.apply_text_document_edit(document_edit, offset_encoding)
                            }
                        };
                        result.map_err(|kind| ApplyEditError {
                            kind,
                            failed_change_idx: i,
                        })?;
                    }
                }
            }

            return Ok(());
        }

        if let Some(ref changes) = workspace_edit.changes {
            log::debug!("workspace changes: {:?}", changes);
            for (i, (uri, text_edits)) in changes.iter().enumerate() {
                let text_edits = text_edits
                    .iter()
                    .cloned()
                    .map(lsp::DocumentTextEdit::Plain)
                    .collect();
                self.apply_text_edits(uri, None, text_edits, offset_encoding)
                    .map_err(|kind| ApplyEditError {
                        kind,
                        failed_change_idx: i,
                    })?;
            }
        }

        crate::features::linked_editing::refresh(self);
        Ok(())
    }

    fn apply_text_document_edit(
        &mut self,
        document_edit: &lsp::TextDocumentEdit,
        offset_encoding: OffsetEncoding,
    ) -> Result<(), ApplyEditErrorKind> {
        self.apply_text_edits(
            &document_edit.text_document.uri,
            document_edit.text_document.version,
            document_edit.edits.clone(),
            offset_encoding,
        )
    }

    fn apply_document_resource_op(
        &mut self,
        op: &lsp::ResourceOp,
    ) -> Result<(), ApplyEditErrorKind> {
        use lsp::ResourceOp;
        // NOTE: If `Uri` gets another variant than `Path`, the below `expect`s
        // may no longer be valid.
        match op {
            ResourceOp::Create(op) => {
                let uri = op.uri.to_editor_uri()?;
                let path = uri.as_path().expect("URIs are valid paths");
                let options = op.options.as_ref();
                let existing = ExistingTarget::from_options(
                    options
                        .and_then(|options| options.overwrite)
                        .unwrap_or(false),
                    options
                        .and_then(|options| options.ignore_if_exists)
                        .unwrap_or(false),
                );
                self.create_path(path, false, existing)?;
            }
            ResourceOp::Delete(op) => {
                let uri = op.uri.to_editor_uri()?;
                let path = uri.as_path().expect("URIs are valid paths");
                let ignore_if_not_exists = op
                    .options
                    .as_ref()
                    .is_some_and(|options| options.ignore_if_not_exists.unwrap_or(false));
                let recursive = op
                    .options
                    .as_ref()
                    .and_then(|options| options.recursive)
                    .unwrap_or(false);
                match self.delete_path(path, recursive) {
                    Err(error)
                        if ignore_if_not_exists && error.kind() == std::io::ErrorKind::NotFound => {
                    }
                    result => result?,
                }
            }
            ResourceOp::Rename(op) => {
                let from_uri = op.old_uri.to_editor_uri()?;
                let from = from_uri.as_path().expect("URIs are valid paths");
                let to_uri = op.new_uri.to_editor_uri()?;
                let to = to_uri.as_path().expect("URIs are valid paths");
                // Protocol rename requires an existing source; interactive moves
                // also support documents whose first save has not happened yet.
                std::fs::symlink_metadata(from)?;
                let options = op.options.as_ref();
                let existing = ExistingTarget::from_options(
                    options
                        .and_then(|options| options.overwrite)
                        .unwrap_or(false),
                    options
                        .and_then(|options| options.ignore_if_exists)
                        .unwrap_or(false),
                );
                self.move_path(from, to, existing)?;
            }
        }
        Ok(())
    }
}
