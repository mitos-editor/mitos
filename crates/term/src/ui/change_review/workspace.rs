//! Full-screen, read-only navigation over one immutable comparison. Git reads
//! and diff policy remain in view; this component owns only presentation state.

use std::{path::PathBuf, sync::Arc};

use tui::buffer::{Buffer, BufferExt as _};
use ui_core::{
    input::{Event, MouseEventKind},
    keymap::Lookup,
};
use view::{editor::Action, features::change_review::ReviewComparison, graphics::Rect};

use super::{ReviewLineKind, ReviewRequest, ReviewView};
use crate::{
    compositor::{Component, Context, EventResult},
    keymap::component::{action_hint, DiffAction},
};

pub struct DiffView {
    view: ReviewView,
    status: vcs::FileStatus,
    workspace: PathBuf,
    comparison: ReviewComparison,
    selected: usize,
    height: usize,
}

impl DiffView {
    pub const ID: &'static str = "diff-view";

    pub(super) fn new(
        view: ReviewView,
        status: vcs::FileStatus,
        workspace: PathBuf,
        comparison: ReviewComparison,
    ) -> Self {
        Self {
            view,
            status,
            workspace,
            comparison,
            selected: 0,
            height: 1,
        }
    }

    /// Capture before returning to the event loop, including unsaved text.
    pub fn open(
        editor: &view::Editor,
        status: vcs::FileStatus,
        workspace: PathBuf,
        comparison: ReviewComparison,
    ) -> Self {
        let request =
            ReviewRequest::new(editor, &status, comparison, &workspace).with_fresh_status();
        let title = title(&status, &workspace, comparison);
        let view = ReviewView::loading(
            title,
            request,
            Arc::new(tokio::sync::Semaphore::new(1)),
            editor.redraw().clone(),
        );
        Self::new(view, status, workspace, comparison)
    }

    fn reload(&mut self, cx: &Context) {
        let engine = self.view.engine;
        let request = ReviewRequest::new(cx.editor, &self.status, self.comparison, &self.workspace)
            .with_engine(engine)
            .with_fresh_status();
        // Dropping the old presentation cancels both queued and running work.
        self.view = ReviewView::loading(
            title(&self.status, &self.workspace, self.comparison),
            request,
            self.view.mode_worker.clone(),
            cx.editor.redraw().clone(),
        );
        self.selected = 0;
    }

    fn comparison(&mut self, next: bool, cx: &Context) {
        let buffer = cx
            .editor
            .document_by_path(self.status.path())
            .is_some_and(|doc| !doc.is_binary());
        let modes = if buffer {
            &[
                ReviewComparison::Head,
                ReviewComparison::Staged,
                ReviewComparison::Unstaged,
                ReviewComparison::Buffer,
            ][..]
        } else {
            &[ReviewComparison::Staged, ReviewComparison::Unstaged][..]
        };
        let current = modes
            .iter()
            .position(|mode| *mode == self.comparison)
            .unwrap_or(0);
        self.comparison = modes[(current + if next { 1 } else { modes.len() - 1 }) % modes.len()];
        self.reload(cx);
    }

    fn clamp(&mut self) {
        self.selected = self
            .selected
            .min(self.view.review.lines.len().saturating_sub(1));
        self.view.offset = self.view.offset.min(self.selected);
        if self.selected >= self.view.offset + self.height {
            self.view.offset = self.selected + 1 - self.height;
        }
        self.view.offset = self
            .view
            .offset
            .min(self.view.review.lines.len().saturating_sub(self.height));
    }

    fn page(&mut self, down: bool, amount: usize) {
        if down {
            self.selected = self.selected.saturating_add(amount);
            self.view.offset = self.view.offset.saturating_add(amount);
        } else {
            self.selected = self.selected.saturating_sub(amount);
            self.view.offset = self.view.offset.saturating_sub(amount);
        }
    }

    fn hunk(&mut self, next: bool) {
        let mut rows = self
            .view
            .review
            .lines
            .iter()
            .enumerate()
            .filter_map(|(row, line)| (line.kind == ReviewLineKind::Header).then_some(row));
        let target = if next {
            rows.find(|row| *row > self.selected)
        } else {
            rows.rfind(|row| *row < self.selected)
        };
        if let Some(row) = target {
            self.selected = row;
            self.view.offset = row;
        }
    }

    fn open_file(&mut self, cx: &mut Context, jump: bool) -> EventResult {
        let target = if jump {
            let Some(source) = &self.view.source else {
                return EventResult::Consumed(None);
            };
            let Some(line) = self.view.review.after_line_at(self.selected) else {
                return EventResult::Consumed(None);
            };
            Some((source.clone(), line))
        } else {
            None
        };
        let path = self.status.path();
        // Editor::open would create a new document for a deleted path. Reviewing
        // a deletion must not silently resurrect it, even as an unsaved buffer.
        if cx.editor.document_by_path(path).is_none() && !path.is_file() {
            cx.editor
                .set_error(|| "This file no longer exists; its deleted text remains in the diff");
            return EventResult::Consumed(None);
        }
        let id = match cx.editor.open(path, Action::Load) {
            Ok(id) => id,
            Err(error) => {
                cx.editor
                    .set_error(|| format!("Could not open diff source: {error}"));
                return EventResult::Consumed(None);
            }
        };
        if let Some((source, _)) = &target
            && cx
                .editor
                .document(id)
                .is_none_or(|doc| doc.is_binary() || doc.text() != source.after())
        {
            cx.editor.set_error(|| "The file differs from this snapshot. Refresh or use Open File to view its current contents");
            return EventResult::Consumed(None);
        }
        let focused = cx.editor.tree.focus;
        let (view, doc) = view::current!(cx.editor);
        view.push_jump(doc, (doc.id(), doc.selection(focused).clone()));
        cx.editor.switch(id, Action::Replace);
        if let Some((_, line)) = target {
            let padding = cx.editor.config().scroll.padding;
            let (view, doc) = view::current!(cx.editor);
            let line = line.min(doc.text().len_lines().saturating_sub(1));
            doc.set_selection(
                view.id,
                editor_core::Selection::point(doc.text().line_to_char(line)),
            );
            view.ensure_cursor_in_view_center(doc, padding);
        }
        EventResult::Consumed(Some(Box::new(|compositor, _| {
            compositor.remove(Self::ID);
            compositor.remove(crate::ui::picker::ID);
        })))
    }
}

fn title(
    status: &vcs::FileStatus,
    workspace: &std::path::Path,
    comparison: ReviewComparison,
) -> String {
    format!(
        "{} · {}",
        comparison.label(),
        status
            .path()
            .strip_prefix(workspace)
            .unwrap_or(status.path())
            .display()
    )
}

impl Component for DiffView {
    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
    fn owns_cursor(&self) -> bool {
        true
    }
    fn has_pending_keys(&self) -> bool {
        self.view.keys.is_pending()
    }
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        self.view.poll(cx);
        // Keep the editor's message row visible for navigation errors.
        let area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        surface.clear_with(area, cx.editor.theme.get("ui.background"));
        let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        self.height = body
            .height
            .saturating_sub(self.view.heading_rows() as u16)
            .max(1) as usize;
        self.clamp();
        self.view
            .draw(body, surface, cx, self.view.offset, Some(self.selected));
        if area.height > 1 {
            let keys = &cx.config.current.component_keys.diff;
            let hint = |action| action_hint(keys, &action);
            let help = format!(
                "{} back · {}/{} hunk · {} jump · {} file · {} refresh · {}/{} compare · snapshot",
                hint(DiffAction::Close),
                hint(DiffAction::PreviousHunk),
                hint(DiffAction::NextHunk),
                hint(DiffAction::OpenSource),
                hint(DiffAction::OpenFile),
                hint(DiffAction::Refresh),
                hint(DiffAction::PreviousComparison),
                hint(DiffAction::NextComparison)
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

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let actions = match event {
            Event::Key(key) => match self
                .view
                .keys
                .lookup(&cx.config.current.component_keys.diff, *key)
            {
                Lookup::Actions(actions) => actions,
                // This view owns input: unmapped editing keys must not reach the
                // document or the picker underneath it.
                _ => return EventResult::Consumed(None),
            },
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.page(false, 3),
                    MouseEventKind::ScrollDown => self.page(true, 3),
                    _ => {}
                }
                self.clamp();
                return EventResult::Consumed(None);
            }
            Event::Paste(_) => return EventResult::Consumed(None),
            _ => return EventResult::Ignored(None),
        };
        for action in actions {
            match action {
                DiffAction::Close => {
                    return EventResult::Consumed(Some(Box::new(|compositor, _| {
                        compositor.remove(Self::ID);
                    })))
                }
                DiffAction::MoveUp => self.selected = self.selected.saturating_sub(1),
                DiffAction::MoveDown => self.selected = self.selected.saturating_add(1),
                DiffAction::MoveStart => self.selected = 0,
                DiffAction::MoveEnd => {
                    self.selected = self.view.review.lines.len().saturating_sub(1)
                }
                DiffAction::PageUp => self.page(false, self.height),
                DiffAction::PageDown => self.page(true, self.height),
                DiffAction::NextHunk => self.hunk(true),
                DiffAction::PreviousHunk => self.hunk(false),
                DiffAction::ScrollLeft => {
                    self.view.horizontal = self.view.horizontal.saturating_sub(8)
                }
                DiffAction::ScrollRight => {
                    self.view.horizontal = self
                        .view
                        .horizontal
                        .saturating_add(8)
                        .min(self.view.review.max_line_chars())
                }
                DiffAction::ToggleEngine => self.view.toggle_engine(cx.editor.redraw().clone()),
                DiffAction::Refresh => self.reload(cx),
                DiffAction::PreviousComparison => self.comparison(false, cx),
                DiffAction::NextComparison => self.comparison(true, cx),
                DiffAction::OpenSource => return self.open_file(cx, true),
                DiffAction::OpenFile => return self.open_file(cx, false),
            }
            self.clamp();
        }
        EventResult::Consumed(None)
    }
}

#[cfg(all(test, feature = "integration"))]
mod tests;
