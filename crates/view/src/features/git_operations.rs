//! Git mutation admission and document coordination, shared by frontends.

use crate::{save::FileWriteBlock, Editor};
use anyhow::{ensure, Result};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};
use vcs::operations::{
    CommitSnapshot, FileOperation, GitRepository, PartialSelection, PartialSource,
    PreparedOperation, StashEntry,
};

pub mod partial;

#[derive(Default)]
pub struct GitOperationsHandler {
    busy: Arc<AtomicBool>,
    revision: AtomicU64,
}

impl GitOperationsHandler {
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }
}

struct Admission(Arc<AtomicBool>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub enum GitAction {
    Partial {
        path: PathBuf,
        kind: FileOperation,
        source: PartialSource,
        selection: PartialSelection,
    },
    File(vcs::FileStatus, FileOperation),
    Stash(PathBuf),
    Apply(GitRepository, StashEntry, bool),
    Commit(CommitSnapshot, String),
}

pub struct GitRequest {
    action: GitAction,
    workspace: PathBuf,
    admission: Admission,
    document: Option<(crate::DocumentId, buffer::Snapshot)>,
    review_guard: Option<super::change_review::ReviewGuard>,
}

pub struct PreparedGitAction {
    operation: PreparedOperation,
    request: GitRequest,
}

pub struct GitTask {
    operation: PreparedOperation,
    state: GitTaskState,
}

#[must_use = "Finish the operation on its originating editor to refresh documents and Git state"]
pub struct GitCompletion {
    state: GitTaskState,
    result: Result<()>,
    notice: Option<String>,
}

struct GitTaskState {
    admission: Admission,
    writes: FileWriteBlock,
    changes_worktree: bool,
    label: &'static str,
    next_line: Option<u32>,
}

impl GitRequest {
    /// A review action must retain its displayed repository inputs through admission.
    pub fn with_review_guard(mut self, guard: super::change_review::ReviewGuard) -> Self {
        self.review_guard = Some(guard);
        self
    }

    pub fn capture(editor: &Editor, workspace: PathBuf, action: GitAction) -> Result<Self> {
        Self::capture_with_document(editor, workspace, action, None)
    }

    /// Aggregate reviews can retain a source buffer opened through a lexical path alias.
    pub fn capture_with_document(
        editor: &Editor,
        workspace: PathBuf,
        action: GitAction,
        source_document: Option<crate::DocumentId>,
    ) -> Result<Self> {
        ensure!(
            source_document.is_none_or(|id| editor.document(id).is_some()),
            "Source buffer was closed"
        );
        ensure!(
            editor
                .workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            "Git actions require workspace trust; run :workspace-trust first"
        );
        let busy = &editor.handlers().git_operations.busy;
        let document = match &action {
            GitAction::Partial { path, source, .. } => source_document
                .and_then(|id| editor.document(id))
                .or_else(|| editor.document_by_path(path))
                .map(|doc| {
                    ensure!(
                        doc.path()
                            .is_some_and(|source| stdx::path::canonicalize_existing(source)
                                == stdx::path::canonicalize_existing(path)),
                        "Source buffer belongs to another file"
                    );
                    if let PartialSource::Buffer(text)
                    | PartialSource::Snapshot {
                        text, buffer: true, ..
                    } = source
                    {
                        ensure!(
                            doc.text() == text.trim_start_matches('\u{feff}'),
                            "Source buffer changed since this review; refresh and try again"
                        );
                    }
                    Ok((doc.id(), doc.snapshot()))
                })
                .transpose()?,
            _ => None,
        };
        ensure!(
            busy.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok(),
            "Another Git action is running"
        );
        Ok(Self {
            action,
            workspace,
            admission: Admission(busy.clone()),
            document,
            review_guard: None,
        })
    }

    pub async fn prepare(self) -> Result<PreparedGitAction> {
        tokio::task::spawn_blocking(move || {
            let operation = match &self.action {
                GitAction::Partial {
                    path,
                    kind,
                    source,
                    selection,
                } => GitRepository::discover(path)?.prepare_partial(
                    path,
                    *kind,
                    source,
                    selection.clone(),
                    true,
                )?,
                GitAction::Stash(path) => GitRepository::discover(path)?.prepare_stash(true)?,
                GitAction::File(status, kind) => {
                    ensure!(
                        status.staged.is_some() || status.unstaged.is_some(),
                        "No file changes selected"
                    );
                    GitRepository::discover(status.path())?.prepare_file(status, *kind, true)?
                }
                GitAction::Apply(repo, entry, index) => repo.prepare_apply(entry, *index, true)?,
                GitAction::Commit(snapshot, message) => snapshot.prepare(message, true)?,
            };
            Ok(PreparedGitAction {
                operation,
                request: self,
            })
        })
        .await?
    }
}

impl PreparedGitAction {
    /// Called on the editor mutation path after off-thread preflight, immediately before execution.
    pub fn admit(self, editor: &mut Editor) -> Result<GitTask> {
        ensure!(
            self.request
                .review_guard
                .as_ref()
                .is_none_or(|guard| guard.is_current()),
            "Git state or source changed since this review; refresh and try again"
        );
        ensure!(
            Arc::ptr_eq(
                &self.request.admission.0,
                &editor.handlers().git_operations.busy
            ),
            "Git action belongs to another editor"
        );
        ensure!(
            self.request
                .document
                .as_ref()
                .is_none_or(|(id, snapshot)| editor
                    .document(*id)
                    .is_some_and(|doc| snapshot.is_current(&doc.buffer))),
            "Source buffer changed while preparing Git action; try again"
        );
        ensure!(
            editor
                .workspace_trust
                .query(&self.request.workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            "Workspace trust changed; Git action canceled"
        );
        let writes = editor.block_file_writes(self.operation.affected_paths())?;
        let changes_worktree = self.operation.changes_worktree();
        for doc in editor.documents() {
            let Some(path) = doc.path() else {
                continue;
            };
            if !writes.contains(path) {
                continue;
            }
            ensure!(
                !self.operation.requires_clean_buffers() || !doc.is_modified(),
                "Save or close unsaved buffer '{}' before changing its file on disk",
                doc.display_name()
            );
        }
        let state = GitTaskState {
            admission: self.request.admission,
            writes,
            changes_worktree,
            label: self.operation.label(),
            next_line: self.operation.next_partial_line(),
        };
        Ok(GitTask {
            operation: self.operation,
            state,
        })
    }
}

impl GitTask {
    /// Frontends must retain this future until completion, including during shutdown.
    pub async fn execute(self) -> GitCompletion {
        let (result, notice) = match self.operation.execute_with_notice().await {
            Ok(notice) => (Ok(()), notice),
            Err(error) => (Err(error), None),
        };
        GitCompletion {
            state: self.state,
            result,
            notice,
        }
    }
}

impl GitCompletion {
    pub fn succeeded(&self) -> bool {
        self.result.is_ok()
    }

    pub fn next_partial_line(&self) -> Option<u32> {
        self.result.as_ref().ok().and(self.state.next_line)
    }
    pub fn finish(self, editor: &mut Editor) {
        if !Arc::ptr_eq(
            &self.state.admission.0,
            &editor.handlers().git_operations.busy,
        ) {
            return;
        }
        let mut preserved = false;
        let mut reload_error = None;
        if self.state.changes_worktree {
            // Include buffers opened during the operation. Only clean buffers may be reloaded;
            // edits made after admission remain in memory and can be saved after the write block drops.
            let documents: Vec<_> = editor
                .documents()
                .filter_map(|doc| {
                    let path = doc.path()?;
                    self.state
                        .writes
                        .contains(path)
                        .then(|| (doc.id(), path.to_owned(), doc.is_modified()))
                })
                .collect();
            for (id, path, modified) in documents {
                if modified {
                    preserved = true;
                    continue;
                }
                let result = if path.exists() {
                    editor.reload_document(id)
                } else {
                    editor
                        .close_document(id, false)
                        .map_err(|_| anyhow::anyhow!("Could not close the removed file buffer"))
                };
                if let Err(error) = result {
                    reload_error.get_or_insert_with(|| {
                        format!(
                            "Could not reload {} after Git action: {error}",
                            path.display()
                        )
                    });
                }
            }
        }
        // Errors can still change the index/worktree (notably conflicts during apply).
        super::auto_reload::reload_vcs(editor);
        editor.refresh_project_reviews();
        editor
            .handlers()
            .git_operations
            .revision
            .fetch_add(1, Ordering::Relaxed);
        match self.result {
            Ok(()) if reload_error.is_some() => editor.set_error(|| reload_error.unwrap()),
            Ok(()) => editor.set_status(format!(
                "{} complete{}{}",
                self.state.label,
                if preserved {
                    "; buffer edits were preserved"
                } else {
                    ""
                },
                self.notice
                    .map_or_else(String::new, |notice| format!("; {notice}"))
            )),
            Err(error) => editor.set_error(|| format!("{error}")),
        }
        editor.redraw().request();
    }
}
