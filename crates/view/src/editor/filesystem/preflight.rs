//! User-initiated file operations, with asynchronous language-server preflight.

use std::{collections::VecDeque, fs, io, path::PathBuf, sync::Arc, time::SystemTime};

use anyhow::{ensure, Result};
use editor_core::Uri;
use event::TaskHandle;
use futures_util::{future::BoxFuture, FutureExt};
use lsp_client::{lsp, Client};

use super::ExistingTarget;
use crate::{DocumentId, Editor};

/// An explicit filesystem target, independent of the currently focused document.
pub enum FileOperation {
    Create { path: PathBuf, is_dir: bool },
    Move { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf, recursive: bool },
}

impl FileOperation {
    fn normalize(self) -> Self {
        use stdx::path::canonicalize;
        match self {
            Self::Create { path, is_dir } => Self::Create {
                path: canonicalize(path),
                is_dir,
            },
            Self::Move { from, to } => Self::Move {
                from: canonicalize(from),
                to: canonicalize(to),
            },
            Self::Delete { path, recursive } => Self::Delete {
                path: canonicalize(path),
                recursive,
            },
        }
    }

    fn paths(&self) -> Vec<&std::path::Path> {
        match self {
            Self::Create { path, .. } | Self::Delete { path, .. } => vec![path],
            Self::Move { from, to } => vec![from, to],
        }
    }

    fn request(
        &self,
        server: &Client,
    ) -> Option<BoxFuture<'static, lsp_client::Result<Option<lsp::WorkspaceEdit>>>> {
        match self {
            Self::Create { path, is_dir } => {
                server.will_create(path, *is_dir).map(FutureExt::boxed)
            }
            Self::Move { from, to } => server
                .will_rename(from, to, from.is_dir())
                .map(FutureExt::boxed),
            Self::Delete { path, .. } => server
                .will_delete(path, path.is_dir())
                .map(FutureExt::boxed),
        }
    }

    fn apply(self, editor: &mut Editor) -> io::Result<()> {
        match self {
            Self::Create { path, is_dir } => {
                editor.create_path(&path, is_dir, ExistingTarget::Reject)
            }
            Self::Move { from, to } => editor.move_path(&from, &to, ExistingTarget::Reject),
            Self::Delete { path, recursive } => editor.delete_path(&path, recursive),
        }
    }
}

/// Metadata guards against changes made outside the editor while a server is pending.
#[derive(PartialEq, Eq)]
struct PathState {
    kind: fs::FileType,
    len: u64,
    modified: Option<SystemTime>,
}

fn path_state(path: &std::path::Path) -> io::Result<Option<PathState>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(PathState {
            kind: metadata.file_type(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

struct DocumentState {
    id: DocumentId,
    buffer: buffer::Snapshot,
    uri: Option<Uri>,
}

struct Preflight {
    operation: FileOperation,
    servers: VecDeque<Arc<Client>>,
    cancel: TaskHandle,
}

impl Preflight {
    fn advance(mut self, editor: &mut Editor) -> Result<()> {
        while let Some(server) = self.servers.pop_front() {
            let Some(future) = self.operation.request(&server) else {
                continue;
            };
            let paths = self
                .operation
                .paths()
                .into_iter()
                .map(path_state)
                .collect::<io::Result<Vec<_>>>()?;
            // A preflight edit may touch any open document, including unversioned edits.
            // Capture again after each provider's edits, so our own changes remain valid.
            let documents: Vec<_> = editor
                .documents()
                .map(|doc| DocumentState {
                    id: doc.id(),
                    buffer: doc.snapshot(),
                    uri: doc.uri(),
                })
                .collect();
            let callbacks = editor.callback_sender().clone();
            let owner = callbacks.clone();
            tokio::spawn(async move {
                let Some(result) = event::cancelable_future(future, &self.cancel).await else {
                    return;
                };
                callbacks
                    .send(move |editor| {
                        let result = (|| -> Result<()> {
                            ensure!(
                                owner.same_destination(editor.callback_sender())
                                    && !self.cancel.is_canceled(),
                                "File operation was canceled"
                            );
                            ensure!(
                                editor.config().lsp.enable
                                    && editor
                                        .language_servers
                                        .get_by_id(server.id())
                                        .is_some_and(|current| Arc::ptr_eq(current, &server)),
                                "File operation server has stopped"
                            );
                            ensure!(
                                documents.len() == editor.documents().count()
                                    && documents.iter().all(|state| editor
                                        .document(state.id)
                                        .is_some_and(|doc| state.buffer.is_current(&doc.buffer)
                                            && state.uri == doc.uri())),
                                "Documents changed while preparing file operation; try again"
                            );
                            let current_paths = self
                                .operation
                                .paths()
                                .into_iter()
                                .map(path_state)
                                .collect::<io::Result<Vec<_>>>()?;
                            ensure!(
                                paths == current_paths,
                                "Paths changed while preparing file operation; try again"
                            );
                            match result {
                                Ok(Some(edit)) => {
                                    editor.apply_workspace_edit(server.offset_encoding(), &edit)?
                                }
                                Ok(None) => {}
                                // Preserve the existing best-effort policy for server failures.
                                Err(error) => {
                                    log::error!("File operation preflight failed: {error}")
                                }
                            }
                            self.advance(editor)
                        })();
                        if let Err(error) = result {
                            editor.set_error(|| {
                                format!("Could not complete file operation: {error}")
                            });
                        }
                    })
                    .await;
            });
            return Ok(());
        }
        self.operation.apply(editor)?;
        Ok(())
    }
}

impl Editor {
    /// Request provider edits in order, then apply the resource change and notifications.
    /// Only one user file operation can be pending. Workspace-edit resource changes
    /// execute directly, so server responses never recursively request preflight.
    pub fn request_file_operation(&mut self, operation: FileOperation) -> Result<()> {
        ensure!(
            !self.file_operation.is_running(),
            "A file operation is already pending"
        );
        let operation = operation.normalize();
        if matches!(&operation, FileOperation::Move { from, to } if from == to) {
            return Ok(());
        }
        let servers = self
            .language_servers
            .iter_clients()
            .filter(|server| self.config().lsp.enable && server.is_initialized())
            .cloned()
            .collect();
        Preflight {
            operation,
            servers,
            cancel: self.file_operation.restart(),
        }
        .advance(self)
    }
}
