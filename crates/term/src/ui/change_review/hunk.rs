//! Actions on the immutable source behind the current-change popup.

use super::*;
use crate::compositor::DrawContext;
use crate::keymap::component::{action_hint, DiffAction};
use ui_core::keymap::Lookup;
use view::features::change_review::HunkReviewSnapshot;

pub struct HunkReviewView {
    view: ReviewView,
    snapshot: Arc<HunkReviewSnapshot>,
}

impl HunkReviewView {
    pub fn new(snapshot: Arc<HunkReviewSnapshot>) -> Self {
        Self {
            view: ReviewView::from_shared(
                "Current change · HEAD → buffer".into(),
                snapshot.review.clone(),
            ),
            snapshot,
        }
    }
}

impl Component for HunkReviewView {
    fn keymap_contexts(&self, cx: &Context) -> Vec<crate::keymap::inspect::Context> {
        let mut context = crate::keymap::inspect::Context::new(
            "Current hunk",
            crate::keymap::inspect::component("keys.diff", &cx.config.current.component_keys.diff),
        );
        context.pending = self.view.keys.pending().to_vec();
        vec![context]
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut DrawContext) {
        if area.is_empty() {
            return;
        }
        let mut content = area;
        content.height = content.height.saturating_sub(1);
        self.view.render(content, surface, cx);
        let footer = panel::content_area(area);
        surface.set_stringn(
            footer.x,
            area.bottom().saturating_sub(1),
            format!(
                "{} restore hunk from HEAD · undo with u",
                action_hint(&cx.config.component_keys.diff, &DiffAction::Restore)
            ),
            footer.width as usize,
            cx.editor.theme.get("ui.text.inactive"),
        );
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.view
            .required_size(viewport)
            .map(|(width, height)| (width, height.saturating_add(1)))
    }

    fn has_pending_keys(&self) -> bool {
        self.view.has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        match self
            .view
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
                        action => {
                            self.view.scroll_action(*action);
                        }
                    }
                }
                EventResult::Consumed(None)
            }
        }
    }
}
