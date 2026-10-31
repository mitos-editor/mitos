//! Capture source coordinates, perform bounded repository reads off-thread, and reject stale replies.

use super::change_review::{ReviewComparison, ReviewLineKind};
use crate::{DocumentId, Editor, ViewId};
use anyhow::{ensure, Context, Result};
use editor_core::{ropey, Rope, Selection};
use std::{ops::RangeInclusive, path::PathBuf, sync::Arc};
use stdx::rope::RopeExt as _;

pub struct PermalinkRequest {
    doc: DocumentId,
    view: ViewId,
    snapshot: buffer::Snapshot,
    uri: Option<editor_core::Uri>,
    selection: Selection,
    path: PathBuf,
    workspace: PathBuf,
    trusted: bool,
    text: Rope,
    revision: Option<String>,
    lines: RangeInclusive<usize>,
    worker: Arc<tokio::sync::Semaphore>,
}

pub struct PermalinkResult {
    request: PermalinkRequest,
    result: Result<String>,
}

pub struct CommitPermalinkRequest {
    path: PathBuf,
    workspace: PathBuf,
    revision: String,
    trusted: bool,
}

impl CommitPermalinkRequest {
    pub fn capture(editor: &Editor, revision: String) -> Self {
        let (_, doc) = crate::current_ref!(editor);
        let path = doc
            .display_metadata()
            .source_path
            .map_or_else(stdx::env::current_working_dir, |path| path.to_owned());
        let workspace = if let Some(review) = doc.review() {
            review.workspace()
        } else if let Some(historical) = &doc.historical {
            &historical.workspace
        } else {
            doc.workspace_root()
        }
        .to_owned();
        Self::new(editor, path, workspace, revision)
    }
    pub fn new(editor: &Editor, path: PathBuf, workspace: PathBuf, revision: String) -> Self {
        let trusted = editor
            .workspace_trust
            .query(&workspace, workspace::trust::TrustQuery::Git)
            .is_trusted();
        Self {
            path,
            workspace,
            revision,
            trusted,
        }
    }
    pub async fn run(self, remote: Option<String>) -> Result<CommitPermalinkResult> {
        tokio::task::spawn_blocking(move || {
            let result = vcs::permalink::commit_url(
                &self.path,
                &self.revision,
                remote.as_deref(),
                self.trusted,
            );
            CommitPermalinkResult {
                request: self,
                result,
            }
        })
        .await
        .context("Commit permalink worker failed")
    }
}

pub struct CommitPermalinkResult {
    request: CommitPermalinkRequest,
    result: Result<String>,
}
impl CommitPermalinkResult {
    pub fn resolve(self, editor: &Editor) -> Option<Result<String>> {
        (editor
            .workspace_trust
            .query(&self.request.workspace, workspace::trust::TrustQuery::Git)
            .is_trusted()
            == self.request.trusted)
            .then_some(self.result)
    }
}

impl PermalinkRequest {
    pub fn capture(editor: &Editor) -> Result<Self> {
        let (view, doc) = crate::current_ref!(editor);
        ensure!(!doc.is_binary(), "Permalinks require a text file");
        ensure!(
            doc.selection(view.id).len() == 1,
            "Select one continuous range to create a permalink"
        );
        let (start, end) = doc
            .selection(view.id)
            .primary()
            .line_range(doc.text().slice(..));
        let mut lines = start..=end;
        let mut text = doc.text().clone();
        let mut revision = None;
        let path = if let Some(review) = doc.review() {
            ensure!(
                !review.is_loading(),
                "Wait for the review to finish loading"
            );
            let source = review.source().context("No source snapshot is available")?;
            let before = review
                .row(start)
                .is_some_and(|row| row.kind == ReviewLineKind::Removed);
            let positions = (start..=end)
                .map(|line| {
                    let row = review
                        .row(line)
                        .context("Select source lines, not diff headings")?;
                    if before { row.before } else { row.after }
                        .context("Select lines from one side of the diff")
                })
                .collect::<Result<Vec<_>>>()?;
            ensure!(
                positions.windows(2).all(|pair| pair[1] == pair[0] + 1),
                "Select consecutive source lines"
            );
            lines = positions[0]..=*positions.last().unwrap();
            text = source.text(before).clone();
            if review.comparison() == ReviewComparison::Historical {
                let endpoint = review
                    .permalink_source(before)
                    .context("This historical snapshot has no committed Git source")?;
                revision = Some(endpoint.revision.as_str().to_owned());
                endpoint.path
            } else {
                review.path().to_owned()
            }
        } else if let Some(historical) = &doc.historical {
            let endpoint = historical
                .revision
                .as_ref()
                .context("This historical snapshot has no committed Git source")?;
            revision = Some(endpoint.revision.as_str().to_owned());
            endpoint.path.clone()
        } else {
            doc.path()
                .context("Save this buffer to a path before copying a permalink")?
                .to_owned()
        };
        ensure!(
            text.len() <= vcs::MAX_REVIEW_BYTES
                && text.len_lines(stdx::rope::LINE_TYPE) <= text_diff::MAX_REVIEW_LINES,
            "File too large to map permalink lines"
        );
        let workspace = if let Some(review) = doc.review() {
            review.workspace()
        } else if let Some(historical) = &doc.historical {
            &historical.workspace
        } else {
            doc.workspace_root()
        }
        .to_owned();
        Ok(Self {
            doc: doc.id(),
            view: view.id,
            snapshot: doc.snapshot(),
            uri: doc.uri(),
            selection: doc.selection(view.id).clone(),
            trusted: editor
                .workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            path,
            workspace,
            text,
            revision,
            lines,
            worker: editor.handlers.change_review.worker(),
        })
    }

    pub async fn run(self, remote: Option<String>) -> Result<PermalinkResult> {
        let permit = self
            .worker
            .clone()
            .acquire_owned()
            .await
            .context("Permalink worker unavailable")?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let result = self.compute(remote.as_deref());
            PermalinkResult {
                request: self,
                result,
            }
        })
        .await
        .context("Permalink worker failed")
    }

    fn compute(&self, remote: Option<&str>) -> Result<String> {
        let file =
            vcs::permalink::file_revision(&self.path, self.revision.as_deref(), self.trusted)?;
        let committed = super::change_review::decode(&file.bytes, None)?;
        let lines = map_lines(&committed, &self.text, self.lines.clone())?;
        file.url(Some(lines), remote)
    }
}

impl PermalinkResult {
    pub fn resolve(self, editor: &Editor) -> Option<Result<String>> {
        let request = self.request;
        let doc = editor.document(request.doc)?;
        let view = editor.tree.try_get(request.view)?;
        (view.doc == request.doc
            && editor.tree.focus() == request.view
            && request.snapshot.is_current(&doc.buffer)
            && doc.uri() == request.uri
            && doc.selection(view.id) == &request.selection
            && editor
                .workspace_trust
                .query(&request.workspace, workspace::trust::TrustQuery::Git)
                .is_trusted()
                == request.trusted)
            .then_some(self.result)
    }
}

/// Only unchanged, contiguous committed lines can be shared faithfully.
fn map_lines(
    base: &Rope,
    text: &Rope,
    lines: RangeInclusive<usize>,
) -> Result<RangeInclusive<usize>> {
    ensure!(
        base.len() <= vcs::MAX_REVIEW_BYTES
            && base.len_lines(stdx::rope::LINE_TYPE) <= text_diff::MAX_REVIEW_LINES,
        "Committed file too large to map permalink lines"
    );
    let (start, end) = (*lines.start(), *lines.end());
    ensure!(
        base.len_lines(stdx::rope::LINE_TYPE) == base.len_lines(ropey::LineType::LF)
            && text.len_lines(stdx::rope::LINE_TYPE) == text.len_lines(ropey::LineType::LF),
        "Permalink line mapping is unavailable for Unicode line separators"
    );
    // Git hosting services count LF lines. Ignore CRLF conversion without applying
    // arbitrary worktree filters, which could add lines or replace a Git LFS pointer.
    let base = Rope::from(base.to_string().replace("\r\n", "\n"));
    let text = Rope::from(text.to_string().replace("\r\n", "\n"));
    let mut shift = 0isize;
    for hunk in text_diff::LineDiff::new(base.clone(), text.clone()).hunks() {
        let (from, to) = (hunk.after.start as usize, hunk.after.end as usize);
        ensure!(
            !(from <= end && to > start) && !(from == to && start < from && from <= end),
            "Selected lines contain uncommitted changes; commit them before copying a permalink"
        );
        if to <= start {
            shift += hunk.before.len() as isize - hunk.after.len() as isize;
        }
    }
    let start = start
        .checked_add_signed(shift)
        .context("Selected line has no committed location")?;
    let end = end
        .checked_add_signed(shift)
        .context("Selected line has no committed location")?;
    ensure!(
        end < base.len_lines(stdx::rope::LINE_TYPE),
        "Selected line has no committed location"
    );
    ensure!(
        base.line_to_char_idx(end) < base.len_chars(),
        "Selected line has no committed text"
    );
    Ok(start..=end)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_ranges_map_around_unsaved_insertions_and_deletions() -> Result<()> {
        let base = Rope::from("one\ntwo\nthree\nfour\n");
        assert_eq!(
            map_lines(&base, &Rope::from("new\none\ntwo\nthree\nfour\n"), 2..=3)?,
            1..=2
        );
        assert_eq!(
            map_lines(&base, &Rope::from("one\nthree\nfour\n"), 1..=2)?,
            2..=3
        );
        assert!(map_lines(&base, &Rope::from("one\nchanged\nthree\nfour\n"), 1..=1).is_err());
        assert!(map_lines(&base, &Rope::from("one\nthree\nfour\n"), 0..=1).is_err());
        assert!(map_lines(&base, &base, 4..=4).is_err());
        assert_eq!(
            map_lines(&base, &Rope::from("one\r\ntwo\r\nthree\r\nfour\r\n"), 1..=2)?,
            1..=2
        );
        Ok(())
    }
}
