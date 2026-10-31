//! Git comparison selection, preview caching, and repository invalidation.

use super::{conflicts::ConflictView, load, Pending, ReviewView};
use crate::{
    compositor::{Context, EventResult},
    ui::picker::{
        scroll::{self, PreviewScroll},
        PickerPreview, PreviewAction,
    },
};
use event::TaskController;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{broadcast, oneshot, Semaphore};
use tui::buffer::Buffer;
use view::features::change_review::ReviewContent;
use view::{
    features::change_review::{ReviewComparison, ReviewRequest, ReviewSnapshot},
    graphics::Rect,
};

enum PreviewContent {
    Diff(ReviewView),
    Conflict(Box<ConflictView>),
}
struct FilePreview {
    content: PreviewContent,
}
impl FilePreview {
    fn heading_rows(&self) -> usize {
        match &self.content {
            PreviewContent::Diff(view) => view.heading_rows(),
            PreviewContent::Conflict(_) => 1,
        }
    }
    fn line_count(&self, width: u16, theme: &theme::Theme) -> usize {
        match &self.content {
            PreviewContent::Diff(view) => view.review.lines.len(),
            PreviewContent::Conflict(view) => view.visual_height(width, theme),
        }
    }
    fn draw(&self, area: Rect, surface: &mut Buffer, cx: &Context, offset: usize) {
        match &self.content {
            PreviewContent::Diff(view) => view.draw(area, surface, cx, offset),
            PreviewContent::Conflict(view) => view.draw(area, surface, cx, offset),
        }
    }
}

/// A single active preview bounds retained text and serializes blocking loads.
/// The callback supplies status without coupling the generic picker to Git.
pub struct ChangeReviewPreview<T> {
    status: fn(&T) -> &vcs::FileStatus,
    workspace: PathBuf,
    comparison: Option<ReviewComparison>,
    task: TaskController,
    worker: Arc<Semaphore>,
    pending: Option<Pending>,
    request: Option<Arc<ReviewRequest>>,
    ready: Option<FilePreview>,
    error: Option<String>,
    snapshot: Option<Arc<ReviewSnapshot>>,
    dirty: bool,
    events: Option<broadcast::Receiver<view::file_watcher::Events>>,
    checked: tokio::time::Instant,
    scroll: PreviewScroll,
    review_on_accept: bool,
    opened_document: bool,
}

impl<T> ChangeReviewPreview<T> {
    pub fn new(workspace: PathBuf, status: fn(&T) -> &vcs::FileStatus) -> Self {
        Self {
            status,
            workspace,
            comparison: None,
            task: TaskController::new(),
            worker: Arc::new(Semaphore::new(1)),
            pending: None,
            request: None,
            ready: None,
            error: None,
            snapshot: None,
            dirty: false,
            events: None,
            checked: tokio::time::Instant::now(),
            scroll: PreviewScroll::default(),
            review_on_accept: false,
            opened_document: false,
        }
    }

    fn comparison_for(&self, editor: &view::Editor, status: &vcs::FileStatus) -> ReviewComparison {
        let buffer = editor
            .document_by_path(status.path())
            .is_some_and(|doc| doc.is_modified() && !doc.is_binary());
        // Unmerged entries have stages 1/2/3, not the stage-zero index entry
        // required by staged/unstaged comparisons. Preview the pending result.
        if matches!(status.unstaged, Some(vcs::FileChange::Conflict { .. })) {
            return if buffer {
                ReviewComparison::Head
            } else {
                ReviewComparison::HeadToWorktree
            };
        }
        match self.comparison {
            Some(ReviewComparison::Buffer) if !buffer => ReviewComparison::Unstaged,
            Some(comparison) => comparison,
            None if buffer => ReviewComparison::Buffer,
            None if status.unstaged.is_some() => ReviewComparison::Unstaged,
            None => ReviewComparison::Staged,
        }
    }

    fn stop(&mut self) {
        self.task.cancel();
        self.pending = None;
        self.request = None;
        self.ready = None;
        self.error = None;
        self.snapshot = None;
        self.scroll = PreviewScroll::default();
    }
}

impl<T> PickerPreview<T> for ChangeReviewPreview<T> {
    fn cancel(&mut self) {
        self.stop();
    }
    fn open_on_accept(&self, item: &T) -> bool {
        self.review_on_accept
            || matches!(
                (self.status)(item).change(),
                vcs::FileChange::Deleted { .. }
            )
    }

    fn close_on_open(&self) -> bool {
        self.opened_document
    }

    fn render(&mut self, item: Option<&T>, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        let Some(item) = item else {
            self.stop();
            return;
        };
        let status = (self.status)(item);
        let comparison = self.comparison_for(cx.editor, status);
        if self.events.is_none() {
            self.events = Some(cx.editor.file_watcher.subscribe());
        }
        let mut dirty = false;
        loop {
            match self.events.as_mut().unwrap().try_recv() {
                Ok(events) => {
                    dirty |= events.iter().any(|event| {
                        let path = event.path.as_std_path();
                        event.ty != view::file_watcher::EventType::Tempfile
                            && (self
                                .snapshot
                                .as_ref()
                                .is_some_and(|snapshot| snapshot.affected_by(path))
                                || cx.editor.file_watcher.is_vcs_path(path)
                                || path
                                    .file_name()
                                    .is_some_and(|name| name == ".gitattributes")
                                || path == status.path()
                                || status.path().starts_with(path))
                    })
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => dirty = true,
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.events = Some(cx.editor.file_watcher.subscribe());
                    dirty = true;
                    break;
                }
            }
        }
        let poll = &cx.editor.config().files.reload.poll;
        self.dirty |= dirty;
        let mut request = ReviewRequest::new(cx.editor, status, comparison, &self.workspace);
        if self.comparison.is_none()
            || matches!(status.unstaged, Some(vcs::FileChange::Conflict { .. }))
        {
            request = request.with_conflict_preview();
        }
        let target_changed = self
            .request
            .as_ref()
            .is_none_or(|old| !old.same_target(&request) || !old.is_current(cx.editor));
        if target_changed {
            self.stop();
            self.dirty = true;
        }
        let poll_due =
            poll.enable && self.checked.elapsed() >= Duration::from_millis(poll.interval.max(100));
        if self.pending.is_none() && (target_changed || self.dirty || poll_due) {
            let previous = if self.dirty {
                None
            } else {
                self.snapshot.clone()
            };
            self.dirty = false;
            self.checked = tokio::time::Instant::now();
            let request = Arc::new(request);
            let cancel = self.task.restart();
            let (tx, result) = oneshot::channel();
            self.pending = Some(Pending {
                request: request.clone(),
                result,
            });
            self.request = Some(request.clone());
            load(
                request,
                previous,
                self.worker.clone(),
                cancel,
                tx,
                cx.editor.redraw().clone(),
            );
        }
        if let Some(pending) = &mut self.pending {
            match pending.result.try_recv() {
                Ok(result) => {
                    if pending.request.is_current(cx.editor) {
                        match result {
                            Ok(snapshot) => {
                                let path = status
                                    .path()
                                    .strip_prefix(&self.workspace)
                                    .unwrap_or(status.path())
                                    .display();
                                let content = match &snapshot.content {
                                    ReviewContent::Diff(review) => {
                                        PreviewContent::Diff(ReviewView::from_shared(
                                            format!("{} · {}", comparison.label(), path),
                                            review.clone(),
                                        ))
                                    }
                                    ReviewContent::Conflict(model) => {
                                        PreviewContent::Conflict(Box::new(ConflictView::new(
                                            format!("Conflicts · {path}"),
                                            snapshot.source.conflict_document(
                                                cx.editor.config.clone(),
                                                model.clone(),
                                            ),
                                        )))
                                    }
                                };
                                let view = FilePreview { content };
                                self.ready = Some(view);
                                self.snapshot = Some(snapshot);
                                self.error = None;
                            }
                            Err(error) => {
                                self.ready = None;
                                self.snapshot = None;
                                self.error = Some(error.to_string());
                            }
                        }
                    }
                    self.pending = None;
                    self.checked = tokio::time::Instant::now();
                    if self.dirty {
                        cx.editor.redraw().request();
                    }
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.pending = None;
                    self.error = Some("Diff preview unavailable".into());
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        let content = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        if let Some(view) = &mut self.ready {
            let text = scroll::text_area(content);
            let heading = view.heading_rows() as u16;
            let rows = content.height.saturating_sub(heading);
            self.scroll
                .update(rows, view.line_count(text.width, &cx.editor.theme));
            view.draw(text, surface, cx, self.scroll.offset);
            self.scroll.render(
                Rect::new(
                    content.x,
                    content.y + heading.min(content.height),
                    content.width,
                    rows,
                ),
                surface,
                &cx.editor.theme,
            );
        } else {
            surface.set_stringn(
                area.x,
                area.y,
                comparison.label(),
                area.width as usize,
                cx.editor.theme.get("ui.text.focus"),
            );
            if content.height > 1 {
                surface.set_stringn(
                    area.x,
                    area.y + 1,
                    self.error.as_deref().unwrap_or("Loading diff…"),
                    area.width as usize,
                    cx.editor.theme.get("ui.text.inactive"),
                );
            }
        }
        if area.height > 1 {
            use crate::keymap::component::{action_hint, PickerAction};
            #[cfg(feature = "git")]
            let keys = &cx.config.current.component_keys.git_picker;
            #[cfg(not(feature = "git"))]
            let keys = &cx.config.current.component_keys.picker;
            let hint = |action| {
                action_hint(keys, &action)
                    .replace("<A-left>", "A-←")
                    .replace("<A-right>", "A-→")
                    .replace("<A-up>", "A-↑")
                    .replace("<A-down>", "A-↓")
                    .replace("<A-o>", "A-o")
                    .replace("<A-p>", "A-p")
                    .replace("<A-n>", "A-n")
            };
            let versions = if matches!(status.unstaged, Some(vcs::FileChange::Conflict { .. })) {
                String::new()
            } else {
                format!(
                    " · {}/{} versions",
                    hint(PickerAction::PreviewPrevious),
                    hint(PickerAction::PreviewNext),
                )
            };
            let help = format!(
                "{} open diff{} · {}/{} scroll",
                hint(PickerAction::OpenPreview),
                versions,
                hint(PickerAction::PreviewScrollUp),
                hint(PickerAction::PreviewScrollDown),
            );
            surface.set_stringn(
                area.x,
                area.bottom() - 1,
                help,
                area.width as usize,
                cx.editor.theme.get("ui.text.inactive"),
            );
        }
    }

    fn action(&mut self, item: Option<&T>, action: PreviewAction, cx: &mut Context) -> EventResult {
        self.opened_document = false;
        match action {
            PreviewAction::ScrollUp => self.scroll.scroll(false),
            PreviewAction::ScrollDown => self.scroll.scroll(true),
            PreviewAction::Previous | PreviewAction::Next => {
                let Some(item) = item else {
                    return EventResult::Consumed(None);
                };
                let status = (self.status)(item);
                if matches!(status.unstaged, Some(vcs::FileChange::Conflict { .. })) {
                    return EventResult::Consumed(None);
                }
                let current = self.comparison_for(cx.editor, status);
                let buffer = cx
                    .editor
                    .document_by_path(status.path())
                    .is_some_and(|doc| doc.is_modified() && !doc.is_binary());
                let modes = if buffer {
                    &[
                        ReviewComparison::Staged,
                        ReviewComparison::Unstaged,
                        ReviewComparison::Buffer,
                    ][..]
                } else {
                    &[ReviewComparison::Staged, ReviewComparison::Unstaged][..]
                };
                let index = modes.iter().position(|mode| *mode == current).unwrap_or(0);
                let next = if matches!(action, PreviewAction::Next) {
                    (index + 1) % modes.len()
                } else {
                    (index + modes.len() - 1) % modes.len()
                };
                self.comparison = Some(modes[next]);
                self.stop();
            }
            PreviewAction::Open(action) => {
                self.review_on_accept = true;
                let Some(item) = item else {
                    return EventResult::Consumed(None);
                };
                let status = (self.status)(item).clone();
                let comparison = self.comparison_for(cx.editor, &status);
                let request = ReviewRequest::new(cx.editor, &status, comparison, &self.workspace);
                let ready = self
                    .request
                    .as_ref()
                    .zip(self.ready.as_ref())
                    .filter(|(old, _)| {
                        self.pending.is_none()
                            && !self.dirty
                            && old.same_target(&request)
                            && old.is_current(cx.editor)
                    })
                    .and(self.snapshot.clone());
                cx.editor.open_review(request, action, ready);
                self.opened_document = true;
                return EventResult::Consumed(None);
            }
        }
        EventResult::Consumed(None)
    }
}
