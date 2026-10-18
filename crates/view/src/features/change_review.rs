//! Immutable comparison capture and background work shared by frontends.

use anyhow::{ensure, Result};
use editor_core::Rope;
use std::{path::PathBuf, sync::Arc, time::SystemTime};
use vcs::{ComparisonKind, FileStatus, MAX_REVIEW_BYTES};

use crate::{DocumentId, Editor};

pub use text_diff::{Review, ReviewLineKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewComparison {
    Staged,
    #[default]
    Unstaged,
    Buffer,
}

impl ReviewComparison {
    pub fn label(self) -> &'static str {
        match self {
            Self::Staged => "Staged · HEAD → index",
            Self::Unstaged => "Unstaged · index → working tree",
            Self::Buffer => "Unsaved · index → buffer",
        }
    }
}

/// Capture is cheap: materialization, repository reads and differencing run off-thread.
pub struct ReviewRequest {
    providers: vcs::DiffProviderRegistry,
    status: FileStatus,
    comparison: ReviewComparison,
    trust_full: bool,
    buffer: Option<Rope>,
    document: Option<(DocumentId, i32)>,
    workspace: std::path::PathBuf,
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    modified: SystemTime,
    len: u64,
    symlink: bool,
}

type RepositoryStamp = Vec<(PathBuf, Option<FileStamp>)>;

/// A bounded review and the metadata used to avoid repeating unchanged comparisons.
pub struct ReviewSnapshot {
    pub review: Arc<Review>,
    stamp: RepositoryStamp,
    status: FileStatus,
    comparison: ReviewComparison,
    trust_full: bool,
    document: Option<(DocumentId, i32)>,
}

impl ReviewSnapshot {
    pub fn affected_by(&self, path: &std::path::Path) -> bool {
        self.stamp
            .iter()
            .any(|(watched, _)| watched == path || watched.starts_with(path))
    }
}

impl ReviewRequest {
    pub fn new(
        editor: &Editor,
        status: &FileStatus,
        comparison: ReviewComparison,
        workspace: &std::path::Path,
    ) -> Self {
        let document = editor.document_by_path(status.path());
        let workspace = document
            .map_or(workspace, |doc| doc.workspace_root())
            .to_owned();
        Self {
            providers: editor.diff_providers.clone(),
            status: status.clone(),
            comparison,
            trust_full: editor
                .workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            buffer: (comparison == ReviewComparison::Buffer)
                .then(|| document.map(|doc| doc.text().clone()))
                .flatten(),
            document: document.map(|doc| (doc.id(), doc.version())),
            workspace,
        }
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.trust_full
            == editor
                .workspace_trust
                .query(&self.workspace, workspace::trust::TrustQuery::Git)
                .is_trusted()
            && (self.comparison != ReviewComparison::Buffer
                || self.document.is_some_and(|(id, version)| {
                    editor.document(id).is_some_and(|doc| {
                        doc.version() == version && doc.path() == Some(self.status.path())
                    })
                }))
    }

    pub fn same_target(&self, other: &Self) -> bool {
        self.status == other.status
            && self.comparison == other.comparison
            && self.trust_full == other.trust_full
            && (self.comparison != ReviewComparison::Buffer || self.document == other.document)
    }

    fn stamp(&self) -> RepositoryStamp {
        let directory = self
            .status
            .path()
            .parent()
            .and_then(|path| path.ancestors().find(|path| path.is_dir()))
            .unwrap_or(&self.workspace);
        let mut paths = self.providers.get_watched_paths(directory, self.trust_full);
        paths.extend(
            directory
                .ancestors()
                .map(|path| path.join(".gitattributes")),
        );
        paths.push(self.status.path().to_owned());
        for change in [&self.status.staged, &self.status.unstaged]
            .into_iter()
            .flatten()
        {
            if let vcs::FileChange::Renamed { from_path, .. } = change {
                paths.push(from_path.clone());
            }
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| {
                // Match native event spellings without following a symlink leaf.
                let path = path.parent().zip(path.file_name()).map_or_else(
                    || path.clone(),
                    |(parent, name)| stdx::path::canonicalize_existing(parent).join(name),
                );
                let stamp = std::fs::symlink_metadata(&path).ok().and_then(|metadata| {
                    Some(FileStamp {
                        modified: metadata.modified().ok()?,
                        len: metadata.len(),
                        symlink: metadata.file_type().is_symlink(),
                    })
                });
                (path, stamp)
            })
            .collect()
    }

    /// Polling checks small metadata before decoding or diffing file contents again.
    pub fn compute_cached(
        &self,
        previous: Option<Arc<ReviewSnapshot>>,
    ) -> Result<Arc<ReviewSnapshot>> {
        let stamp = self.stamp();
        if let Some(previous) = previous
            && previous.stamp == stamp
            && previous.status == self.status
            && previous.comparison == self.comparison
            && previous.trust_full == self.trust_full
            && (self.comparison != ReviewComparison::Buffer || previous.document == self.document)
        {
            return Ok(previous);
        }
        let review = Arc::new(self.compute()?);
        ensure!(
            stamp == self.stamp(),
            "Repository changed while loading the diff; refreshing will retry"
        );
        Ok(Arc::new(ReviewSnapshot {
            review,
            stamp,
            status: self.status.clone(),
            comparison: self.comparison,
            trust_full: self.trust_full,
            document: self.document,
        }))
    }

    pub fn compute(&self) -> Result<Review> {
        let kind = match self.comparison {
            ReviewComparison::Staged => ComparisonKind::Staged,
            ReviewComparison::Buffer => ComparisonKind::Index,
            ReviewComparison::Unstaged => ComparisonKind::Unstaged,
        };
        let comparison = self
            .providers
            .read_comparison(&self.status, kind, self.trust_full)?;
        let (before, after) = if self.comparison == ReviewComparison::Buffer {
            let buffer = self
                .buffer
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No open text buffer for this file"))?;
            ensure!(
                buffer.len_bytes() <= MAX_REVIEW_BYTES,
                "Buffer too large to review (limit 10 MiB)"
            );
            (decode(&comparison.before)?, buffer)
        } else {
            (decode(&comparison.before)?, decode(&comparison.after)?)
        };
        Ok(Review::new(before, after, None))
    }
}

fn decode(bytes: &[u8]) -> Result<Rope> {
    ensure!(
        !stdx::file::is_binary(bytes),
        "Binary file: text diff unavailable"
    );
    let (text, _, _) = text_io::from_reader(&mut &*bytes, None)?;
    ensure!(
        text.len_bytes() <= MAX_REVIEW_BYTES,
        "Decoded file too large to review (limit 10 MiB)"
    );
    Ok(text)
}

/// A hunk inspection belongs to the exact selection that requested it.
pub struct HunkReviewRequest {
    target: super::request::SelectionRequest,
    worker: Arc<tokio::sync::Semaphore>,
    providers: vcs::DiffProviderRegistry,
    path: std::path::PathBuf,
    workspace: std::path::PathBuf,
    trust_full: bool,
    text: Rope,
    line: usize,
}

impl HunkReviewRequest {
    pub fn capture(editor: &Editor) -> Result<Self> {
        let (view, doc) = crate::current_ref!(editor);
        ensure!(
            !doc.is_binary(),
            "A text buffer is required to inspect a change"
        );
        let path = doc
            .path()
            .ok_or_else(|| {
                anyhow::anyhow!("Save this buffer to a path before inspecting Git changes")
            })?
            .to_owned();
        ensure!(
            doc.text().len_bytes() <= MAX_REVIEW_BYTES,
            "Buffer too large to review (limit 10 MiB)"
        );
        let cancel = editor.handlers.change_review.lock().restart();
        Ok(Self {
            target: super::request::SelectionRequest::new(doc, view.id, cancel, Vec::new()),
            worker: editor.handlers.change_review_worker.clone(),
            providers: editor.diff_providers.clone(),
            path,
            workspace: doc.workspace_root().to_owned(),
            trust_full: editor
                .workspace_trust
                .query(doc.workspace_root(), workspace::trust::TrustQuery::Git)
                .is_trusted(),
            text: doc.text().clone(),
            line: doc.cursor_line(view.id),
        })
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.target
            .get(editor)
            .is_some_and(|(view, _)| editor.tree.focus == view.id)
            && editor
                .workspace_trust
                .query(&self.workspace, workspace::trust::TrustQuery::Git)
                .is_trusted()
                == self.trust_full
    }

    /// Serialize blocking hunk work per editor. Cancellation drops queued requests immediately;
    /// a running worker retains its permit until its blocking computation actually finishes.
    pub async fn run(self: Arc<Self>) -> Result<Review> {
        let worker = self.worker.clone();
        let cancel = self.target.cancel_handle().clone();
        run_hunk(worker, cancel, move || self.compute()).await
    }

    pub fn compute(&self) -> Result<Review> {
        ensure!(
            !self.target.cancel_handle().is_canceled(),
            "Change inspection cancelled"
        );
        let status = FileStatus {
            staged: None,
            unstaged: Some(vcs::FileChange::Modified {
                path: self.path.clone(),
            }),
        };
        let comparison =
            self.providers
                .read_comparison(&status, ComparisonKind::Head, self.trust_full)?;
        Ok(Review::new(
            decode(&comparison.before)?,
            self.text.clone(),
            Some(self.line),
        ))
    }
}

async fn run_hunk<T: Send + 'static>(
    worker: Arc<tokio::sync::Semaphore>,
    cancel: event::TaskHandle,
    compute: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let work = async {
        let permit = worker.acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            compute()
        })
        .await?
    };
    event::cancelable_future(work, &cancel)
        .await
        .ok_or_else(|| anyhow::anyhow!("Change inspection canceled"))?
}

pub(crate) fn register_hooks() {
    use crate::events::DocumentFocusLost;
    event::register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        event.editor.handlers.change_review.lock().cancel();
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canceled_hunk_workers_keep_their_permit_and_queued_work_is_discarded() {
        let worker = Arc::new(tokio::sync::Semaphore::new(1));
        let mut task = event::TaskController::new();
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let first = tokio::spawn(run_hunk(worker.clone(), task.restart(), move || {
            let _ = started.send(());
            blocked
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            Ok(())
        }));
        running.await.unwrap();
        let queued = tokio::spawn(run_hunk(
            worker.clone(),
            task.restart(),
            || -> Result<()> { panic!("canceled queued work must never start") },
        ));
        assert!(first.await.unwrap().is_err());
        assert_eq!(worker.available_permits(), 0);
        let current = task.restart();
        assert!(queued.await.unwrap().is_err());
        release.send(()).unwrap();
        assert_eq!(run_hunk(worker, current, || Ok(42)).await.unwrap(), 42);
    }
}
