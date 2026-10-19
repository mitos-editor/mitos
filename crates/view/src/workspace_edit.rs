//! Workspace edits shared by server requests, code actions, rename, and save actions.
//!
//! Ordered changes run in protocol order and stop at the first failure. Completed document
//! edits and filesystem operations remain applied, matching the advertised LSP
//! `abort` failure-handling capability.

use std::path::PathBuf;

use crate::language_servers::edits::generate_transaction_from_edits;
use editor_core::Uri;
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
    InvalidUrl(#[from] editor_core::uri::UrlConversionError),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

impl Editor {
    fn apply_text_edits(
        &mut self,
        url: &lsp_client::Url,
        version: Option<i32>,
        text_edits: Vec<lsp::TextEdit>,
        offset_encoding: OffsetEncoding,
    ) -> Result<(), ApplyEditErrorKind> {
        let uri = match Uri::try_from(url) {
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
        let transaction = generate_transaction_from_edits(doc.text(), text_edits, offset_encoding)?;
        self.apply_document_changes(doc_id, actual_version, transaction.changes())?;
        Ok(())
    }

    /// Apply changes in order, stopping on failure without rolling back earlier changes.
    /// A failed index refers to `document_changes`, or to iteration order in `changes`.
    pub fn apply_workspace_edit(
        &mut self,
        offset_encoding: OffsetEncoding,
        workspace_edit: &lsp::WorkspaceEdit,
    ) -> Result<(), ApplyEditError> {
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
                let text_edits = text_edits.to_vec();
                self.apply_text_edits(uri, None, text_edits, offset_encoding)
                    .map_err(|kind| ApplyEditError {
                        kind,
                        failed_change_idx: i,
                    })?;
            }
        }

        Ok(())
    }

    fn apply_text_document_edit(
        &mut self,
        document_edit: &lsp::TextDocumentEdit,
        offset_encoding: OffsetEncoding,
    ) -> Result<(), ApplyEditErrorKind> {
        let edits = document_edit
            .edits
            .iter()
            .map(|edit| match edit {
                lsp::OneOf::Left(edit) => edit.clone(),
                lsp::OneOf::Right(edit) => edit.text_edit.clone(),
            })
            .collect();
        self.apply_text_edits(
            &document_edit.text_document.uri,
            document_edit.text_document.version,
            edits,
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
                let uri = Uri::try_from(&op.uri)?;
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
                let uri = Uri::try_from(&op.uri)?;
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
                let from_uri = Uri::try_from(&op.old_uri)?;
                let from = from_uri.as_path().expect("URIs are valid paths");
                let to_uri = Uri::try_from(&op.new_uri)?;
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
