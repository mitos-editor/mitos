//! Actions on the immutable source behind the current-change popup.

use super::*;
use crate::keymap::component::{action_hint, DiffAction};
use ui_core::keymap::{KeymapState, Lookup};
use view::features::change_review::HunkReviewSnapshot;

pub struct HunkReviewView {
    view: ReviewView,
    snapshot: Arc<HunkReviewSnapshot>,
    keys: KeymapState,
}

impl HunkReviewView {
    fn stage(&self, cx: &mut Context, unstage: bool, advance: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.snapshot.is_current(cx.editor),
            "Change inspection is stale; inspect the change again before staging"
        );
        #[cfg(feature = "git")]
        {
            let kind = if unstage {
                vcs::operations::FileOperation::Unstage
            } else {
                vcs::operations::FileOperation::Stage
            };
            let (request, next) =
                view::features::git_operations::partial::capture_cursor_hunk(cx.editor, kind)?;
            let captured = (
                request.with_review_guard(self.snapshot.action_guard()),
                next,
            );
            crate::commands::start_partial_git(cx.editor, cx.jobs, captured, advance);
            Ok(())
        }
        #[cfg(not(feature = "git"))]
        {
            let _ = (unstage, advance);
            anyhow::bail!("No Git support included in this build")
        }
    }
    pub fn new(snapshot: Arc<HunkReviewSnapshot>) -> Self {
        Self {
            view: ReviewView::from_shared(
                "Current change · HEAD → buffer".into(),
                snapshot.review.clone(),
            ),
            snapshot,
            keys: KeymapState::default(),
        }
    }
}

impl Component for HunkReviewView {
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        if area.is_empty() {
            return;
        }
        let mut content = area;
        content.height = content.height.saturating_sub(2);
        self.view.render(content, surface, cx);
        let footer = panel::content_area(area);
        let keys = &cx.config.current.component_keys.diff;
        let rows = [
            format!(
                "{} restore hunk from HEAD (undo in buffer) · {} stage + next",
                action_hint(keys, &DiffAction::Restore),
                action_hint(keys, &DiffAction::StageAndNext)
            ),
            format!(
                "{} stage: index → buffer · {} unstage: HEAD → index",
                action_hint(keys, &DiffAction::Stage),
                action_hint(keys, &DiffAction::Unstage)
            ),
        ];
        let count = area.height.min(2);
        for (row, text) in rows.iter().take(count as usize).enumerate() {
            surface.set_stringn(
                footer.x,
                area.bottom() - count + row as u16,
                text,
                footer.width as usize,
                cx.editor.theme.get("ui.text.inactive"),
            );
        }
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.view
            .required_size(viewport)
            .map(|(width, height)| (width, height.saturating_add(2)))
    }

    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending() || self.view.has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        match self
            .keys
            .lookup(&cx.config.current.component_keys.diff, *key)
        {
            Lookup::Pending => EventResult::Consumed(None),
            Lookup::Unbound => EventResult::Ignored(None),
            Lookup::Actions(actions) => {
                for action in actions {
                    match action {
                        DiffAction::Restore => {
                            if let Err(error) = self.snapshot.restore(cx.editor) {
                                cx.editor.set_error(|| error.to_string());
                            }
                            return EventResult::Consumed(Some(Box::new(|compositor, _| {
                                compositor.remove(ReviewView::ID);
                            })));
                        }
                        action @ (DiffAction::Stage
                        | DiffAction::Unstage
                        | DiffAction::StageAndNext) => {
                            if let Err(error) = self.stage(
                                cx,
                                *action == DiffAction::Unstage,
                                *action == DiffAction::StageAndNext,
                            ) {
                                cx.editor.set_error(|| error.to_string());
                            }
                            return EventResult::Consumed(Some(Box::new(|compositor, _| {
                                compositor.remove(ReviewView::ID);
                            })));
                        }
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
                    }
                }
                EventResult::Consumed(None)
            }
        }
    }
}
