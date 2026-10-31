//! Repository-wide snapshots assembled from the existing bounded file-review model.

use std::{ops::Range, path::PathBuf, sync::Arc, time::SystemTime};

use anyhow::{ensure, Result};
use editor_core::{Rope, Selection};
use event::{TaskController, TaskHandle};
use stdx::rope::RopeExt as _;
use text_diff::{ReviewLine, ReviewLineKind};
use vcs::{ChangedFileScope, FileChange, FileStatus};

use super::change_review::{ReviewComparison, ReviewContent, ReviewRequest, ReviewSnapshot};
use crate::{editor::Action, Document, DocumentId, Editor};

const MAX_FILES: usize = 500;
const MAX_BYTES: usize = vcs::MAX_REVIEW_BYTES;

pub struct ProjectReviewDocument {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub comparison: ReviewComparison,
    pub sections: Vec<ProjectSection>,
    pub loading: bool,
    complete: bool,
    task: TaskController,
    advance: Option<ProjectAdvance>,
}

pub(crate) struct ProjectAdvance {
    pub(crate) view: crate::ViewId,
    pub(crate) snapshot: buffer::Snapshot,
    pub(crate) selection: Selection,
    pub(crate) path: PathBuf,
    pub(crate) line: Option<u32>,
}

pub struct ProjectSection {
    pub status: FileStatus,
    pub comparison: ReviewComparison,
    pub start_line: usize,
    pub snapshot: Option<Arc<ReviewSnapshot>>,
    rows: Range<usize>,
    request: Arc<ReviewRequest>,
}

type RepositoryStamp = Vec<(PathBuf, Option<(SystemTime, u64)>)>;

struct Load {
    id: DocumentId,
    root: PathBuf,
    comparison: ReviewComparison,
    requests: Vec<(FileStatus, Arc<ReviewRequest>)>,
    truncated: bool,
    cancel: TaskHandle,
    stamp: RepositoryStamp,
}

impl ProjectSection {
    pub fn source_document(&self) -> Option<DocumentId> {
        self.request.document_id()
    }

    pub fn row(&self, document_line: usize) -> Option<&ReviewLine> {
        let ReviewContent::Diff(review) = &self.snapshot.as_ref()?.content else {
            return None;
        };
        review
            .lines
            .get(document_line.checked_sub(self.start_line + 1)?)
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.request.is_current(editor)
    }
}

impl ProjectReviewDocument {
    pub fn hunk_rows(&self) -> Vec<usize> {
        self.sections
            .iter()
            .flat_map(|section| {
                section
                    .snapshot
                    .iter()
                    .filter_map(|snapshot| match &snapshot.content {
                        ReviewContent::Diff(review) => Some(review),
                        ReviewContent::Conflict(_) => None,
                    })
                    .flat_map(move |review| {
                        review
                            .lines
                            .iter()
                            .enumerate()
                            .filter_map(move |(row, line)| {
                                (line.kind == ReviewLineKind::Header)
                                    .then_some(section.start_line + 1 + row)
                            })
                    })
            })
            .collect()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }
    pub fn is_complete(&self) -> bool {
        self.complete
            && !self.loading
            && self
                .sections
                .iter()
                .all(|section| section.snapshot.is_some())
    }

    pub fn section(&self, line: usize) -> Option<&ProjectSection> {
        self.sections
            .iter()
            .find(|section| section.rows.contains(&line))
    }
}

impl Document {
    pub fn project_review(&self) -> Option<&ProjectReviewDocument> {
        self.project_review.as_deref()
    }
}

impl Editor {
    pub fn open_project_review(
        &mut self,
        root: PathBuf,
        workspace: PathBuf,
        comparison: ReviewComparison,
        action: Action,
    ) -> DocumentId {
        let mut doc = Document::from(
            Rope::from("Loading project changes…\n"),
            None,
            self.config.clone(),
            self.syn_loader.load_full(),
        )
        .with_readonly_name(format!(
            "{} [project diff: {}]",
            root.display(),
            comparison.name()
        ));
        doc.project_review = Some(Box::new(ProjectReviewDocument {
            root,
            workspace,
            comparison,
            sections: Vec::new(),
            loading: true,
            complete: false,
            task: TaskController::new(),
            advance: None,
        }));
        let id = self.new_file_from_document(action, doc);
        self.start_project_review(id);
        id
    }

    pub fn refresh_project_review(&mut self) -> Result<()> {
        let id = crate::current_ref!(self).1.id();
        ensure!(
            self.document(id)
                .is_some_and(|doc| doc.project_review.is_some()),
            "Current buffer is not a project review"
        );
        self.start_project_review(id);
        Ok(())
    }

    /// Editor Git actions refresh their derived project snapshots after completion.
    pub fn refresh_project_reviews(&mut self) {
        let ids = self
            .documents()
            .filter(|doc| doc.project_review().is_some())
            .map(Document::id)
            .collect::<Vec<_>>();
        for id in ids {
            self.start_project_review(id);
        }
    }

    fn start_project_review(&mut self, id: DocumentId) {
        let state = self
            .documents
            .get_mut(&id)
            .unwrap()
            .project_review
            .as_mut()
            .unwrap();
        let cancel = state.task.restart();
        state.loading = true;
        state.complete = false;
        state.advance = None;
        let (root, workspace, comparison) = (
            state.root.clone(),
            state.workspace.clone(),
            state.comparison,
        );
        let trusted = self
            .workspace_trust
            .query(&workspace, workspace::trust::TrustQuery::Git)
            .is_trusted();
        let providers = self.diff_providers.clone();
        let callbacks = self.handlers.callbacks.clone();
        let document_paths = self
            .documents()
            .filter_map(|doc| doc.path().map(|path| (doc.id(), path.to_owned())))
            .collect::<Vec<_>>();
        tokio::spawn(async move {
            let metadata_provider = providers.clone();
            let metadata_root = root.clone();
            let (stamp, aliases) = tokio::task::spawn_blocking(move || {
                let aliases = document_paths
                    .into_iter()
                    .map(|(id, path)| {
                        let path = match (path.parent(), path.file_name()) {
                            (Some(parent), Some(name)) => {
                                stdx::path::canonicalize_existing(parent).join(name)
                            }
                            _ => path,
                        };
                        // Keep the editor spelling, including simplified Windows prefixes.
                        (id, stdx::path::normalize(path))
                    })
                    .collect::<Vec<_>>();
                (
                    repository_stamp(metadata_provider.get_watched_paths(&metadata_root, trusted)),
                    aliases,
                )
            })
            .await
            .unwrap_or_default();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let check = cancel.clone();
            let visited = std::sync::atomic::AtomicUsize::new(0);
            providers.for_each_file_status(
                ChangedFileScope::Repository(root.clone()),
                trusted,
                move |_, result| {
                    if let Ok(status) = &result
                        && ((comparison == ReviewComparison::Staged && status.staged.is_none())
                            || (comparison == ReviewComparison::Unstaged
                                && status.unstaged.is_none()))
                    {
                        return true;
                    }
                    !check.is_canceled()
                        && visited.fetch_add(1, std::sync::atomic::Ordering::Relaxed) <= MAX_FILES
                        && tx.send(result).is_ok()
                },
            );
            let mut statuses = Vec::new();
            let mut failure = None;
            while let Some(status) = rx.recv().await {
                match status {
                    Ok(status) => statuses.push(status),
                    Err(error) => {
                        failure = Some(error.to_string());
                        break;
                    }
                }
                if statuses.len() > MAX_FILES {
                    break;
                }
            }
            drop(rx);
            if cancel.is_canceled() {
                return;
            }
            callbacks.send(move |editor| {
                if cancel.is_canceled() || editor.document(id).is_none() { return; }
                if let Some(failure) = failure {
                    editor.finish_project_review(id, &cancel, format!("Could not read project changes: {failure}\nUse :vcs-project-refresh to retry.\n"), Vec::new(), false);
                    return;
                }
                if editor.workspace_trust.query(&workspace, workspace::trust::TrustQuery::Git).is_trusted() != trusted {
                    editor.finish_project_review(id, &cancel, "Workspace trust changed while loading; use :vcs-project-refresh to retry.\n".into(), Vec::new(), false);
                    return;
                }
                let root = stdx::path::normalize(root);
                if comparison == ReviewComparison::Buffer {
                    for (id, path) in &aliases {
                        if path.starts_with(&root) && editor.document(*id).is_some_and(|doc| doc.is_modified())
                            && !statuses.iter().any(|status| status.path() == path) {
                            statuses.push(FileStatus { staged: None, unstaged: Some(FileChange::Modified { path: path.to_owned() }) });
                        }
                    }
                }
                statuses.retain(|status| match comparison {
                    ReviewComparison::Staged => status.staged.is_some(),
                    ReviewComparison::Unstaged => status.unstaged.is_some(),
                    _ => true,
                });
                statuses.sort_by(|a,b| a.path().cmp(b.path()));
                let truncated = statuses.len() > MAX_FILES;
                statuses.truncate(MAX_FILES);
                let requests = statuses.into_iter().map(|status| {
                    let document = aliases.iter().find(|(_, path)| path == status.path()).and_then(|(id, _)| editor.document(*id));
                    let request = Arc::new(ReviewRequest::for_document(editor, &status, comparison, &workspace, document));
                    (status, request)
                }).collect::<Vec<_>>();
                editor.compute_project_review(Load { id, root, comparison, requests, truncated, cancel, stamp });
            }).await;
        });
    }

    fn compute_project_review(&self, load: Load) {
        let Load {
            id,
            root,
            comparison,
            requests,
            truncated,
            cancel,
            stamp,
        } = load;
        let callbacks = self.handlers.callbacks.clone();
        tokio::spawn(async move {
            let mut text = format!("Repository: {}\n{}\nSnapshot · :vcs-project-refresh updates it · Enter opens this file's review\n\n", literal(&root.display().to_string()), comparison.label());
            let mut sections = Vec::new();
            let mut snapshot_bytes = 0;
            let mut line = 4;
            let mut output_limit = false;
            let mut source_limit = false;
            let mut file_limit = false;
            for (status, request) in requests {
                if cancel.is_canceled() {
                    return;
                }
                if text.len() >= MAX_BYTES {
                    text.push_str("Project review shortened at 10 MiB; review remaining files individually.\n");
                    break;
                }
                let start = line;
                text.push_str(&format!(
                    "--- {} ---\n",
                    literal(
                        &status
                            .path()
                            .strip_prefix(&root)
                            .unwrap_or(status.path())
                            .display()
                            .to_string()
                    )
                ));
                line += 1;
                let snapshot = match request
                    .clone()
                    .run(None, cancel.clone())
                    .await
                    .and_then(|snapshot| snapshot.diff().map(|review| (snapshot, review)))
                {
                    Ok((snapshot, review)) => {
                        file_limit |= review.truncated;
                        snapshot_bytes += snapshot.source.byte_len();
                        if snapshot_bytes > 64 * 1024 * 1024 {
                            text.push_str("Project review shortened at 64 MiB of source snapshots; review remaining files individually.\n");
                            source_limit = true;
                            break;
                        }
                        for row in &review.lines {
                            if text.len() + row.text.len() + 2 > MAX_BYTES {
                                text.push_str("Project review shortened at 10 MiB; review remaining files individually.\n");
                                line += 1;
                                output_limit = true;
                                break;
                            }
                            let marker = match row.kind {
                                ReviewLineKind::Added => '+',
                                ReviewLineKind::Removed => '-',
                                ReviewLineKind::Context => ' ',
                                _ => '@',
                            };
                            text.push(marker);
                            text.push_str(&literal(&row.text));
                            text.push('\n');
                            line += 1;
                        }
                        Some(snapshot)
                    }
                    Err(error) => {
                        text.push_str(&format!("Cannot review: {}\n", literal(&error.to_string())));
                        line += 1;
                        None
                    }
                };
                text.push('\n');
                line += 1;
                sections.push(ProjectSection {
                    status,
                    comparison,
                    start_line: start,
                    snapshot,
                    rows: start..line,
                    request,
                });
                if output_limit {
                    break;
                }
            }
            if sections.is_empty() {
                text.push_str("No changes in this comparison.\n");
            }
            if truncated {
                text.push_str("Only the first 500 changed files are shown; review remaining files individually.\n");
            }
            let expected_stamp = stamp.clone();
            let unchanged = tokio::task::spawn_blocking(move || {
                repository_stamp(expected_stamp.into_iter().map(|(path, _)| path).collect())
                    == stamp
            })
            .await
            .unwrap_or(false);
            callbacks
                .send(move |editor| {
                    if !unchanged {
                        editor.finish_project_review(
                            id,
                            &cancel,
                            "Git state changed while loading; use :vcs-project-refresh to retry.\n"
                                .into(),
                            Vec::new(),
                            false,
                        );
                    } else if sections.iter().any(|section| !section.is_current(editor)) {
                        editor.finish_project_review(
                            id,
                            &cancel,
                            "Buffers changed while loading; use :vcs-project-refresh to retry.\n"
                                .into(),
                            Vec::new(),
                            false,
                        );
                    } else {
                        editor.finish_project_review(
                            id,
                            &cancel,
                            text,
                            sections,
                            !truncated && !output_limit && !source_limit && !file_limit,
                        );
                    }
                })
                .await;
        });
    }

    fn finish_project_review(
        &mut self,
        id: DocumentId,
        cancel: &TaskHandle,
        text: String,
        sections: Vec<ProjectSection>,
        complete: bool,
    ) {
        if cancel.is_canceled() {
            return;
        }
        let Some(doc) = self.documents.get_mut(&id) else {
            return;
        };
        let advance = doc
            .project_review
            .as_mut()
            .unwrap()
            .advance
            .take()
            .filter(|advance| {
                advance.snapshot.is_current(&doc.buffer)
                    && doc.try_selection(advance.view) == Some(&advance.selection)
                    && self.tree.focus() == advance.view
                    && self
                        .tree
                        .try_get(advance.view)
                        .is_some_and(|view| view.doc == id)
            });
        let transaction = text_diff::compare_ropes(doc.text(), &Rope::from(text));
        if let Err(error) = doc.replace_readonly_content(transaction.changes(), true) {
            self.set_error(|| error.to_string());
            return;
        }
        let state = doc.project_review.as_mut().unwrap();
        state.sections = sections;
        state.loading = false;
        state.complete = complete;
        if let Some(advance) = advance
            && let Some(line) = advance.target(state)
        {
            doc.set_selection(
                advance.view,
                Selection::point(doc.text().line_to_char_idx(line)),
            );
            let padding = self.config.load().scroll.padding;
            self.tree
                .get_mut(advance.view)
                .ensure_cursor_in_view(doc, padding);
        }
        self.redraw().request();
    }

    /// Resolve an aggregate stage-and-next target against the replacement rows.
    /// A manual refresh or newer cursor movement cancels this continuation.
    #[cfg(feature = "git")]
    pub(crate) fn advance_project_review_after_refresh(
        &mut self,
        document: DocumentId,
        advance: ProjectAdvance,
    ) {
        if let Some(state) = self
            .documents
            .get_mut(&document)
            .and_then(|doc| doc.project_review.as_mut())
            && state.loading
        {
            state.advance = Some(advance);
        }
    }

    pub fn navigate_project_review(&mut self, forward: bool, files: bool) -> Result<()> {
        let (view, doc) = crate::current_ref!(self);
        let state = doc
            .project_review()
            .ok_or_else(|| anyhow::anyhow!("Current buffer is not a project review"))?;
        let line = doc.cursor_line(view.id);
        let targets = if files {
            state
                .sections
                .iter()
                .map(|section| section.start_line)
                .collect::<Vec<_>>()
        } else {
            state.hunk_rows()
        };
        ensure!(!targets.is_empty(), "No changes to navigate");
        let target = if forward {
            targets
                .iter()
                .find(|target| **target > line)
                .or(targets.first())
        } else {
            targets
                .iter()
                .rev()
                .find(|target| **target < line)
                .or(targets.last())
        }
        .copied()
        .unwrap();
        let scrolloff = self.config().scroll.padding;
        let (view, doc) = crate::current!(self);
        doc.set_selection(
            view.id,
            Selection::point(doc.text().line_to_char_idx(target)),
        );
        view.ensure_cursor_in_view(doc, scrolloff);
        Ok(())
    }

    pub fn open_project_review_file(&mut self) -> Result<()> {
        let (view, doc) = crate::current_ref!(self);
        let state = doc
            .project_review()
            .ok_or_else(|| anyhow::anyhow!("Current buffer is not a project review"))?;
        let section = state
            .section(doc.cursor_line(view.id))
            .ok_or_else(|| anyhow::anyhow!("Move to a file in the project review"))?;
        let request = ReviewRequest::for_document(
            self,
            &section.status,
            section.comparison,
            &state.workspace,
            section.source_document().and_then(|id| self.document(id)),
        )
        .with_fresh_status();
        self.open_review(request, Action::Replace, None);
        Ok(())
    }
}

impl ProjectAdvance {
    fn target(&self, project: &ProjectReviewDocument) -> Option<usize> {
        if let Some(section) = project
            .sections
            .iter()
            .find(|section| section.status.path() == self.path)
            && let Some(snapshot) = &section.snapshot
            && let Ok(review) = snapshot.diff()
        {
            let mut header = None;
            let mut first = None;
            for (row, mapped) in review.lines.iter().enumerate() {
                if mapped.kind == ReviewLineKind::Header {
                    header = Some(section.start_line + 1 + row);
                    first = first.or(header);
                }
                if self
                    .line
                    .is_some_and(|line| mapped.after == Some(line as usize))
                    && header.is_some()
                {
                    return header;
                }
            }
            if first.is_some() {
                return first;
            }
        }
        let first_hunk = |section: &ProjectSection| {
            section
                .snapshot
                .as_ref()?
                .diff()
                .ok()?
                .lines
                .iter()
                .position(|row| row.kind == ReviewLineKind::Header)
                .map(|row| section.start_line + 1 + row)
        };
        project
            .sections
            .iter()
            .filter(|section| section.status.path() > self.path.as_path())
            .find_map(first_hunk)
            .or_else(|| project.sections.iter().find_map(first_hunk))
    }
}

fn literal(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch != '\t' && (ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}')) {
                '�'
            } else {
                ch
            }
        })
        .collect()
}

fn repository_stamp(paths: Vec<PathBuf>) -> RepositoryStamp {
    paths
        .into_iter()
        .map(|path| {
            let stamp = std::fs::symlink_metadata(&path)
                .ok()
                .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));
            (path, stamp)
        })
        .collect()
}
