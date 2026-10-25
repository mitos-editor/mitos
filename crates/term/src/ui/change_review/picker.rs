//! Git comparison selection, preview caching, and repository invalidation.

use super::{load, Pending, ReviewView};
use crate::{
    compositor::{Context, EventResult},
    ui::picker::{PickerPreview, PreviewAction},
};
use event::TaskController;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{broadcast, oneshot, Semaphore};
use tui::buffer::Buffer;
use view::{
    features::change_review::{ReviewComparison, ReviewRequest, ReviewSnapshot},
    graphics::Rect,
};

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
    ready: Option<ReviewView>,
    error: Option<String>,
    snapshot: Option<Arc<ReviewSnapshot>>,
    dirty: bool,
    events: Option<broadcast::Receiver<view::file_watcher::Events>>,
    checked: tokio::time::Instant,
    height: u16,
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
            height: 1,
            review_on_accept: false,
            opened_document: false,
        }
    }

    fn comparison_for(&self, editor: &view::Editor, status: &vcs::FileStatus) -> ReviewComparison {
        let buffer = editor
            .document_by_path(status.path())
            .is_some_and(|doc| doc.is_modified() && !doc.is_binary());
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
        self.height = area.height;
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
        let request = ReviewRequest::new(cx.editor, status, comparison, &self.workspace);
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
                                let offset = self.ready.as_ref().map_or(0, |view| view.offset);
                                let mut view = ReviewView::from_shared(
                                    format!(
                                        "{} · {}",
                                        comparison.label(),
                                        status
                                            .path()
                                            .strip_prefix(&self.workspace)
                                            .unwrap_or(status.path())
                                            .display()
                                    ),
                                    snapshot.review.clone(),
                                );
                                view.offset = offset;
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
            view.offset =
                view.offset.min(view.review.lines.len().saturating_sub(
                    content.height.saturating_sub(view.heading_rows() as u16) as usize,
                ));
            view.draw(content, surface, cx, view.offset);
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
            let help = format!(
                "{} open diff · {}/{} versions · {}/{} scroll",
                hint(PickerAction::OpenPreview),
                hint(PickerAction::PreviewPrevious),
                hint(PickerAction::PreviewNext),
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
            PreviewAction::ScrollUp => {
                if let Some(view) = &mut self.ready {
                    view.offset = view
                        .offset
                        .saturating_sub((self.height / 2).max(1) as usize);
                }
            }
            PreviewAction::ScrollDown => {
                if let Some(view) = &mut self.ready {
                    view.offset = view
                        .offset
                        .saturating_add((self.height / 2).max(1) as usize);
                }
            }
            PreviewAction::Previous | PreviewAction::Next => {
                let Some(item) = item else {
                    return EventResult::Consumed(None);
                };
                let status = (self.status)(item);
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
