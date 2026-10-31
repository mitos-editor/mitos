//! Immutable comparison capture and background work shared by frontends.

use anyhow::{ensure, Result};
use editor_core::{
    encoding::{self, Encoding},
    Rope,
};
use std::{path::PathBuf, sync::Arc, time::SystemTime};
use vcs::{ComparisonKind, FileStatus, MAX_REVIEW_BYTES};

use crate::{DocumentId, Editor};

pub use language_config::DiffEngine;
pub use text_diff::{Review, ReviewLineKind};

mod document;
mod engine;
mod live;
pub(crate) use live::{
    handle_file_events, has_live_reviews, invalidate, poll, Handler, ReviewTrigger,
};
pub(crate) mod historical;
pub use document::ReviewDocument;
pub use engine::ReviewSource;
use engine::Settings;
pub use historical::{HistoricalReviewRequest, HistoricalReviewSnapshot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewComparison {
    Staged,
    /// Immutable historical sources, independent of today's index and working tree.
    Historical,
    #[default]
    Unstaged,
    Buffer,
    /// HEAD compared with the current editor buffer, including unsaved edits.
    Head,
    /// HEAD compared with the saved working file, independent of open buffers.
    HeadToWorktree,
}

impl ReviewComparison {
    pub fn name(self) -> &'static str {
        match self {
            Self::Historical => "revision",
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
            Self::Buffer => "buffer",
            Self::Head | Self::HeadToWorktree => "HEAD",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Historical => "Historical comparison",
            Self::Staged => "Staged · HEAD → index",
            Self::Unstaged => "Unstaged · index → working tree",
            Self::Buffer => "Unsaved · index → buffer",
            Self::Head => "Changes · HEAD → buffer",
            Self::HeadToWorktree => "Changes · HEAD → working tree",
        }
    }

    fn uses_buffer(self) -> bool {
        matches!(self, Self::Buffer | Self::Head)
    }
}

/// Capture is cheap: materialization, repository reads and differencing run off-thread.
#[derive(Clone)]
pub struct ReviewRequest {
    worker: Arc<tokio::sync::Semaphore>,
    providers: vcs::DiffProviderRegistry,
    status: FileStatus,
    fresh_status: bool,
    prefer_conflicts: bool,
    comparison: ReviewComparison,
    trust_full: bool,
    buffer: Option<Rope>,
    document: Option<(DocumentId, i32)>,
    encoding: Option<&'static Encoding>,
    workspace: std::path::PathBuf,
    settings: Settings,
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    modified: SystemTime,
    len: u64,
    symlink: bool,
}

type RepositoryStamp = Vec<(PathBuf, Option<FileStamp>)>;

/// Picker previews can present unresolved source text without computing a diff.
pub enum ReviewContent {
    Diff(Arc<Review>),
    Conflict(Arc<super::conflicts::ConflictPresentation>),
}

/// A bounded review and the metadata used to avoid repeating unchanged comparisons.
pub struct ReviewSnapshot {
    pub content: ReviewContent,
    pub source: Arc<ReviewSource>,
    stamp: RepositoryStamp,
    request: ReviewRequest,
}

impl ReviewSnapshot {
    pub fn diff(&self) -> Result<Arc<Review>> {
        match &self.content {
            ReviewContent::Diff(review) => Ok(review.clone()),
            ReviewContent::Conflict(_) => anyhow::bail!("Conflict preview is not a diff"),
        }
    }

    pub fn engine(&self) -> DiffEngine {
        self.request.engine()
    }

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
            worker: editor.handlers.change_review.worker.clone(),
            providers: editor.diff_providers.clone(),
            status: status.clone(),
            fresh_status: false,
            prefer_conflicts: false,
            comparison,
            trust_full: editor
                .workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            buffer: comparison
                .uses_buffer()
                .then(|| document.map(|doc| doc.text().clone()))
                .flatten(),
            document: document.map(|doc| (doc.id(), doc.version())),
            encoding: document.map(|doc| doc.encoding()),
            workspace,
            settings: Settings::capture(editor, status.path()),
        }
    }

    /// Prefer structured conflict sections for this picker preview only.
    pub fn with_conflict_preview(mut self) -> Self {
        self.prefer_conflicts = true;
        self
    }

    /// Re-resolve renames and conflicts when opening directly or refreshing.
    pub fn with_fresh_status(mut self) -> Self {
        self.fresh_status = true;
        self
    }

    pub fn engine(&self) -> DiffEngine {
        self.settings.engine
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.settings.is_current(editor, self.status.path())
            && self.encoding
                == editor
                    .document_by_path(self.status.path())
                    .map(|doc| doc.encoding())
            && self.trust_full
                == editor
                    .workspace_trust
                    .query(&self.workspace, workspace::trust::TrustQuery::Git)
                    .is_trusted()
            && (!self.comparison.uses_buffer()
                || match self.document {
                    Some((id, version)) => editor.document(id).is_some_and(|doc| {
                        doc.version() == version && doc.path() == Some(self.status.path())
                    }),
                    None => editor.document_by_path(self.status.path()).is_none(),
                })
    }

    pub fn same_target(&self, other: &Self) -> bool {
        self.prefer_conflicts == other.prefer_conflicts
            && self.status == other.status
            && self.fresh_status == other.fresh_status
            && self.encoding == other.encoding
            && self.settings.current(&other.settings)
            && self.comparison == other.comparison
            && self.trust_full == other.trust_full
            && (!self.comparison.uses_buffer() || self.document == other.document)
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
        self.compute_cached_cancellable(previous, &|| false)
    }

    pub fn compute_cached_cancellable(
        &self,
        previous: Option<Arc<ReviewSnapshot>>,
        canceled: &dyn Fn() -> bool,
    ) -> Result<Arc<ReviewSnapshot>> {
        ensure!(!canceled(), "Diff preview canceled");
        let stamp = self.stamp();
        if let Some(previous) = previous
            && previous.stamp == stamp
            && previous.request.same_target(self)
        {
            return Ok(previous);
        }
        let source = Arc::new(self.source()?);
        ensure!(!canceled(), "Diff preview canceled");
        let conflicts = self
            .prefer_conflicts
            .then(|| super::conflicts::ConflictPresentation::new(source.after()));
        let content = if let Some(conflicts) = conflicts.filter(|conflicts| conflicts.active()) {
            source.prepare_syntax(canceled);
            ReviewContent::Conflict(Arc::new(conflicts))
        } else {
            ReviewContent::Diff(source.compute(Some(self.engine()), canceled))
        };
        ensure!(!canceled(), "Diff preview canceled");
        ensure!(
            stamp == self.stamp(),
            "Repository changed while loading the diff; refreshing will retry"
        );
        Ok(Arc::new(ReviewSnapshot {
            content,
            source,
            stamp,
            request: self.clone(),
        }))
    }

    pub fn compute(&self) -> Result<Arc<Review>> {
        Ok(self.source()?.compute(None, &|| false))
    }

    /// Run under the editor's bounded review budget, shared with hunk and
    /// historical reads. Canceling a running read retains its permit until exit.
    pub async fn run(
        self: Arc<Self>,
        previous: Option<Arc<ReviewSnapshot>>,
        cancel: event::TaskHandle,
    ) -> Result<Arc<ReviewSnapshot>> {
        let worker = self.worker.clone();
        let check = cancel.clone();
        run_review(worker, cancel, move || {
            self.compute_cached_cancellable(previous, &|| check.is_canceled())
        })
        .await
    }

    fn source(&self) -> Result<ReviewSource> {
        let kind = match self.comparison {
            ReviewComparison::Historical => {
                anyhow::bail!("Historical sources require a captured revision")
            }
            ReviewComparison::Staged => ComparisonKind::Staged,
            ReviewComparison::Buffer if self.buffer.is_some() => ComparisonKind::Index,
            ReviewComparison::Buffer => ComparisonKind::Unstaged,
            ReviewComparison::Unstaged => ComparisonKind::Unstaged,
            ReviewComparison::Head if self.buffer.is_some() => ComparisonKind::Head,
            ReviewComparison::Head | ReviewComparison::HeadToWorktree => {
                ComparisonKind::HeadToWorktree
            }
        };
        let status = if self.fresh_status {
            self.providers
                .file_status(self.status.path(), self.trust_full)?
                .unwrap_or_else(|| FileStatus {
                    staged: None,
                    unstaged: Some(vcs::FileChange::Modified {
                        path: self.status.path().to_owned(),
                    }),
                })
        } else {
            self.status.clone()
        };
        let comparison = self
            .providers
            .read_comparison(&status, kind, self.trust_full)?;
        let (before, after) = if let Some(buffer) = self.buffer.clone() {
            ensure!(
                buffer.len() <= MAX_REVIEW_BYTES,
                "Buffer too large to review (limit 10 MiB)"
            );
            (decode(&comparison.before, self.encoding)?, buffer)
        } else {
            (
                decode(&comparison.before, self.encoding)?,
                decode(&comparison.after, self.encoding)?,
            )
        };
        Ok(ReviewSource::new(
            before,
            after,
            None,
            self.settings.clone(),
        ))
    }
}

pub(crate) fn decode(bytes: &[u8], encoding: Option<&'static Encoding>) -> Result<Rope> {
    ensure!(
        bytes.len() <= MAX_REVIEW_BYTES,
        "File too large to review (limit 10 MiB)"
    );
    let explicitly_unicode = encoding
        .is_some_and(|encoding| encoding == encoding::UTF_16LE || encoding == encoding::UTF_16BE);
    ensure!(
        !stdx::file::has_binary_signature(bytes)
            && (explicitly_unicode || !stdx::file::is_binary(bytes)),
        "Binary file: text diff unavailable"
    );
    let (text, _, _) = text_io::from_reader(&mut &*bytes, encoding)?;
    ensure!(
        text.len() <= MAX_REVIEW_BYTES,
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
    encoding: &'static Encoding,
    line: usize,
    settings: Settings,
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
            doc.text().len() <= MAX_REVIEW_BYTES,
            "Buffer too large to review (limit 10 MiB)"
        );
        let cancel = editor.handlers.change_review.hunk.lock().restart();
        Ok(Self {
            target: super::request::SelectionRequest::new(doc, view.id, cancel, Vec::new()),
            worker: editor.handlers.change_review.worker.clone(),
            providers: editor.diff_providers.clone(),
            path: path.clone(),
            workspace: doc.workspace_root().to_owned(),
            trust_full: editor
                .workspace_trust
                .query(doc.workspace_root(), workspace::trust::TrustQuery::Git)
                .is_trusted(),
            text: doc.text().clone(),
            encoding: doc.encoding(),
            line: doc.cursor_line(view.id),
            settings: Settings::capture(editor, &path),
        })
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.settings.is_current(editor, &self.path)
            && self.target.get(editor).is_some_and(|(view, doc)| {
                editor.tree.focus() == view.id && doc.encoding() == self.encoding
            })
            && editor
                .workspace_trust
                .query(&self.workspace, workspace::trust::TrustQuery::Git)
                .is_trusted()
                == self.trust_full
    }

    /// Serialize blocking hunk work per editor. Cancellation drops queued requests immediately;
    /// a running worker retains its permit until its blocking computation actually finishes.
    pub async fn run(self: Arc<Self>) -> Result<(Arc<ReviewSource>, Arc<Review>)> {
        let worker = self.worker.clone();
        let cancel = self.target.cancel_handle().clone();
        run_review(worker, cancel, move || self.compute_review()).await
    }

    pub fn compute(&self) -> Result<Arc<Review>> {
        self.compute_review().map(|(_, review)| review)
    }

    pub fn compute_review(&self) -> Result<(Arc<ReviewSource>, Arc<Review>)> {
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
        let source = Arc::new(ReviewSource::new(
            decode(&comparison.before, Some(self.encoding))?,
            self.text.clone(),
            Some(self.line),
            self.settings.clone(),
        ));
        let review = source.compute(None, &|| self.target.cancel_handle().is_canceled());
        ensure!(
            !self.target.cancel_handle().is_canceled(),
            "Change inspection canceled"
        );
        Ok((source, review))
    }
}

async fn run_review<T: Send + 'static>(
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
    live::register_hooks();
    use crate::events::DocumentFocusLost;
    event::register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        event.editor.handlers.change_review.hunk.lock().cancel();
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
        let first = tokio::spawn(run_review(worker.clone(), task.restart(), move || {
            let _ = started.send(());
            blocked
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            Ok(())
        }));
        running.await.unwrap();
        let queued = tokio::spawn(run_review(
            worker.clone(),
            task.restart(),
            || -> Result<()> { panic!("canceled queued work must never start") },
        ));
        assert!(first.await.unwrap().is_err());
        assert_eq!(worker.available_permits(), 0);
        let current = task.restart();
        assert!(queued.await.unwrap().is_err());
        release.send(()).unwrap();
        assert_eq!(run_review(worker, current, || Ok(42)).await.unwrap(), 42);
    }
}
