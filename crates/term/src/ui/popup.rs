use crate::keymap::component::PopupAction;
use crate::{
    commands::Open,
    compositor::{Callback, Component, Context, Event, EventResult},
    key,
    ui::panel::{self, PopupBorder},
};
use tui::buffer::BufferExt as _;
use tui::{buffer::Buffer as Surface, scrollbar, widgets::Widget};
use ui_core::keymap::{Binding, KeymapState, Lookup};

use text_layout::DisplayPosition;
use ui_core::input::{MouseEvent, MouseEventKind};
use view::{graphics::Rect, Editor};

const MAX_HEIGHT: u16 = 26;
const MAX_WIDTH: u16 = 120;

struct RenderInfo {
    area: Rect,
    child_height: u16,
    render_borders: bool,
}

#[derive(Debug, Clone, Copy, Default)]
enum PopupKind {
    #[default]
    Popup,
    Menu,
}

// TODO: share logic with Menu, it's essentially Popup(render_fn), but render fn needs to return
// a width/height hint. maybe Popup(Box<Component>)

pub struct Popup<T: Component> {
    keys: KeymapState,
    contents: T,
    position: Option<DisplayPosition>,
    area: Rect,
    position_bias: Open,
    scroll_half_pages: usize,
    auto_close: bool,
    ignore_escape_key: bool,
    id: &'static str,
    has_scrollbar: bool,
    kind: PopupKind,
    border: PopupBorder,
}

impl<T: Component> Popup<T> {
    pub fn new(id: &'static str, contents: T) -> Self {
        Self {
            contents,
            keys: KeymapState::default(),
            position: None,
            position_bias: Open::Below,
            area: Rect::new(0, 0, 0, 0),
            scroll_half_pages: 0,
            auto_close: false,
            ignore_escape_key: false,
            id,
            has_scrollbar: true,
            kind: PopupKind::default(),
            border: PopupBorder::default(),
        }
    }

    /// Set the anchor position next to which the popup should be drawn.
    ///
    /// Note that this is not the position of the top-left corner of the rendered popup itself,
    /// but rather the screen-space position of the information to which the popup refers.
    pub fn position(mut self, pos: Option<DisplayPosition>) -> Self {
        self.position = pos;
        self
    }

    pub fn get_position(&self) -> Option<DisplayPosition> {
        self.position
    }

    /// Set the popup to prefer to render above or below the anchor position.
    ///
    /// This preference will be ignored if the viewport doesn't have enough space in the
    /// chosen direction.
    pub fn position_bias(mut self, bias: Open) -> Self {
        self.position_bias = bias;
        self
    }

    pub fn auto_close(mut self, auto_close: bool) -> Self {
        self.auto_close = auto_close;
        self
    }

    /// Ignores an escape keypress event, letting the outer layer
    /// (usually the editor) handle it. This is useful for popups
    /// in insert mode like completion and signature help where
    /// the popup is closed on the mode change from insert to normal
    /// which is done with the escape key. Otherwise the popup consumes
    /// the escape key event and closes it, and an additional escape
    /// would be required to exit insert mode.
    pub fn ignore_escape_key(mut self, ignore: bool) -> Self {
        self.ignore_escape_key = ignore;
        self
    }

    pub fn scroll_half_page_down(&mut self) {
        self.scroll_half_pages += 1;
    }

    pub fn scroll_half_page_up(&mut self) {
        self.scroll_half_pages = self.scroll_half_pages.saturating_sub(1);
    }

    /// Toggles the Popup's scrollbar.
    /// Consider disabling the scrollbar in case the child
    /// already has its own.
    pub fn with_scrollbar(mut self, enable_scrollbar: bool) -> Self {
        self.has_scrollbar = enable_scrollbar;
        self
    }

    /// Follow this feature's current border preference on each render.
    pub fn with_border(mut self, border: PopupBorder) -> Self {
        self.border = border;
        self
    }

    pub fn menu_style(mut self) -> Self {
        self.kind = PopupKind::Menu;
        self
    }

    pub fn contents(&self) -> &T {
        &self.contents
    }

    pub fn contents_mut(&mut self) -> &mut T {
        &mut self.contents
    }

    pub fn area(&mut self, viewport: Rect, editor: &Editor) -> Rect {
        self.render_info(viewport, editor).area
    }

    fn render_info(&mut self, viewport: Rect, editor: &Editor) -> RenderInfo {
        let mut position = editor.cursor().0.unwrap_or_default();
        if let Some(old_position) = self
            .position
            .filter(|old_position| old_position.row == position.row)
        {
            position = old_position;
        } else {
            self.position = Some(position);
        }

        self.render_info_at(viewport, position)
    }

    fn render_info_at(&mut self, viewport: Rect, position: DisplayPosition) -> RenderInfo {
        // Cursor positions are absolute screen coordinates. Clamp before narrowing
        // to u16 so a stale anchor cannot escape a resized viewport.
        let anchor_y = position.row.clamp(
            viewport.y as usize,
            viewport.bottom().saturating_sub(1).max(viewport.y) as usize,
        ) as u16;
        let above = anchor_y.saturating_sub(viewport.y);
        let below = viewport.bottom().saturating_sub(anchor_y.saturating_add(1));

        // Measure using the larger side first. Measuring only in the preferred
        // direction lets height-constrained children hide how much space they need.
        let mut max_height = above.max(below).min(MAX_HEIGHT);
        let mut max_width = viewport.width.saturating_sub(2).min(MAX_WIDTH);
        let render_borders = max_height > 3 && max_width > 3;
        let border = if render_borders { 2 } else { 0 };
        max_width -= border;
        max_height -= border;
        let (width, child_height) = self
            .contents
            .required_size((max_width, max_height))
            .expect("Component needs required_size implemented in order to be embedded in a popup");
        let height = child_height.saturating_add(border).min(MAX_HEIGHT);

        // Keep the preference when the popup fits. Otherwise use the side with
        // more room, retaining the preference on ties to keep placement stable.
        let put_above = match self.position_bias {
            Open::Above => height <= above || above >= below,
            Open::Below => height > below && above > below,
        };
        let height = height.min(if put_above { above } else { below });
        let width = width.min(max_width).saturating_add(border);
        let right = viewport.right().saturating_sub(viewport.width.min(2));
        let x = position.col.clamp(
            viewport.x as usize,
            right.saturating_sub(width).max(viewport.x) as usize,
        ) as u16;
        let y = if put_above {
            anchor_y - height
        } else {
            anchor_y.saturating_add(1).min(viewport.bottom())
        };
        let area = Rect::new(x, y, width, height);
        RenderInfo {
            area,
            child_height,
            render_borders,
        }
    }

    fn handle_mouse_event(
        &mut self,
        &MouseEvent {
            kind,
            column: x,
            row: y,
            ..
        }: &MouseEvent,
    ) -> EventResult {
        if self.auto_close && matches!(kind, MouseEventKind::Down(_)) {
            let close_fn: Callback = Box::new(|compositor, _| {
                // remove the layer
                compositor.remove(self.id.as_ref());
            });

            return EventResult::Ignored(Some(close_fn));
        }

        let mouse_is_within_popup = x >= self.area.left()
            && x < self.area.right()
            && y >= self.area.top()
            && y < self.area.bottom();

        if !mouse_is_within_popup {
            return EventResult::Ignored(None);
        }

        match kind {
            MouseEventKind::ScrollDown if self.has_scrollbar => {
                self.scroll_half_page_down();
                EventResult::Consumed(None)
            }
            MouseEventKind::ScrollUp if self.has_scrollbar => {
                self.scroll_half_page_up();
                EventResult::Consumed(None)
            }
            _ => EventResult::Ignored(None),
        }
    }
}

impl<T: Component> Component for Popup<T> {
    #[cfg(feature = "integration")]
    fn has_pending_render(&mut self) -> bool {
        self.contents.has_pending_render()
    }

    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending() || self.contents.has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let key = match event {
            Event::Key(event) => *event,
            Event::Mouse(event) => return self.handle_mouse_event(event),
            Event::Resize(_, _) => {
                // TODO: calculate inner area, call component's handle_event with that area
                return EventResult::Ignored(None);
            }
            Event::Paste(_) => {
                self.keys.clear();
                return self.contents.handle_event(event, cx);
            }
            _ => return EventResult::Ignored(None),
        };

        if key!(Esc) == key
            && self.ignore_escape_key
            && !self.has_pending_keys()
            && matches!(cx.config.current.component_keys.popup.0.get(&key), Some(Binding::Actions(actions)) if actions == &[PopupAction::Cancel])
        {
            return EventResult::Ignored(None);
        }

        let id = self.id;
        let close_fn: Callback = Box::new(move |compositor, _| {
            compositor.remove(id.as_ref());
        });

        // A pending popup prefix owns its next key. Otherwise, nested menus
        // and page selectors have priority over their container's bindings.
        let child_callback = if self.keys.is_pending() {
            None
        } else {
            match self.contents.handle_event(event, cx) {
                EventResult::Consumed(callback) => return EventResult::Consumed(callback),
                EventResult::Ignored(callback) => callback,
            }
        };
        match self
            .keys
            .lookup(&cx.config.current.component_keys.popup, key)
        {
            Lookup::Pending => EventResult::Consumed(None),
            Lookup::Actions(actions) => {
                for &action in actions {
                    match action {
                        PopupAction::Cancel => return EventResult::Consumed(Some(close_fn)),
                        PopupAction::ScrollDown => self.scroll_half_page_down(),
                        PopupAction::ScrollUp => self.scroll_half_page_up(),
                    }
                }
                EventResult::Consumed(None)
            }
            Lookup::Unbound => {
                if self.auto_close {
                    EventResult::Ignored(Some(close_fn))
                } else {
                    EventResult::Ignored(child_callback)
                }
            }
        }
    }

    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let RenderInfo {
            area,
            child_height,
            render_borders,
        } = self.render_info(viewport, cx.editor);
        self.area = area;

        // clear area
        let background = if matches!(self.kind, PopupKind::Menu) {
            // TODO: consistently style menu
            cx.editor
                .theme
                .try_get("ui.menu")
                .unwrap_or_else(|| cx.editor.theme.get("ui.text"))
        } else {
            cx.editor.theme.get("ui.popup")
        };
        surface.clear_with(area, background);

        let mut inner = area;
        if render_borders {
            let block = panel::bordered(&cx.editor.theme, self.border.resolve(&cx.editor.config()));
            inner = block.inner(area);
            Widget::render(block, area, surface);
        }
        let max_offset = child_height.saturating_sub(inner.height) as usize;
        let half_page_size = (inner.height / 2) as usize;
        let scroll = clamp_scroll(&mut self.scroll_half_pages, half_page_size, max_offset);
        cx.scroll = Some(scroll);
        self.contents.render(inner, surface, cx);

        if self.has_scrollbar && child_height > inner.height {
            let scroll_style = cx.editor.theme.get("ui.menu.scroll");
            scrollbar::render(
                surface,
                Rect {
                    y: inner.y,
                    height: inner.height,
                    ..area
                },
                inner.height,
                child_height as usize,
                scroll,
                render_borders,
                scroll_style,
            );
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(self.id)
    }
}

/// Resolves a scroll request in half pages to a line offset, clamped to
/// `max_offset`, and writes the clamp back so scrolling up after overshooting
/// the end responds at once. The write-back rounds up: rounding down would
/// make the next frame resolve to an offset short of `max_offset`, snapping
/// the popup back up (all the way to the top when the overflow is under half
/// a page).
fn clamp_scroll(half_pages: &mut usize, half_page_size: usize, max_offset: usize) -> usize {
    let scroll = max_offset.min(half_pages.saturating_mul(half_page_size));
    if half_page_size > 0 {
        *half_pages = scroll.div_ceil(half_page_size);
    }
    scroll
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Text;

    #[test]
    fn clamped_scroll_is_stable_across_frames() {
        // 5 lines of overflow, half page of 10: one scroll reaches the end.
        let mut half_pages = 1;
        assert_eq!(clamp_scroll(&mut half_pages, 10, 5), 5);
        assert_eq!(clamp_scroll(&mut half_pages, 10, 5), 5);

        // Overshooting by several half pages still lands on the end and
        // one step up leaves it.
        let mut half_pages = 4;
        assert_eq!(clamp_scroll(&mut half_pages, 10, 15), 15);
        assert_eq!(clamp_scroll(&mut half_pages, 10, 15), 15);
        half_pages -= 1;
        assert_eq!(clamp_scroll(&mut half_pages, 10, 15), 10);
    }

    #[test]
    fn content_that_fits_does_not_scroll() {
        let mut half_pages = 3;
        assert_eq!(clamp_scroll(&mut half_pages, 10, 0), 0);
        assert_eq!(half_pages, 0);
    }

    fn text_popup(lines: usize, bias: Open) -> Popup<Text> {
        Popup::new("test", Text::new(vec!["line"; lines].join("\n"))).position_bias(bias)
    }

    #[test]
    fn preferred_side_is_kept_when_content_fits_including_borders() {
        let viewport = Rect::new(0, 0, 80, 30);
        for (bias, row, expected_y) in [(Open::Below, 22, 23), (Open::Above, 6, 0)] {
            let info =
                text_popup(4, bias).render_info_at(viewport, DisplayPosition { row, col: 4 });
            assert_eq!(info.area, Rect::new(4, expected_y, 6, 6));
        }
    }

    #[test]
    fn above_preference_flips_below_when_it_has_more_room() {
        let info = text_popup(16, Open::Above)
            .render_info_at(Rect::new(0, 0, 80, 30), DisplayPosition { row: 7, col: 4 });
        assert_eq!(info.area, Rect::new(4, 8, 6, 18));
    }

    #[test]
    fn equal_space_preserves_preference_and_height_cap() {
        for (bias, expected_y) in [(Open::Above, 4), (Open::Below, 31)] {
            let info = text_popup(50, bias)
                .render_info_at(Rect::new(0, 0, 80, 61), DisplayPosition { row: 30, col: 4 });
            assert_eq!(info.area, Rect::new(4, expected_y, 6, MAX_HEIGHT));
        }
    }

    #[test]
    fn wrapped_content_is_measured_before_choosing_a_side() {
        let mut popup = Popup::new("test", Text::new("x".repeat(60)));
        let info =
            popup.render_info_at(Rect::new(0, 0, 14, 30), DisplayPosition { row: 22, col: 0 });
        assert_eq!(info.area, Rect::new(0, 14, 12, 8));
    }

    #[test]
    fn short_popup_fits_below_even_with_fewer_than_six_rows() {
        let info = text_popup(1, Open::Below)
            .render_info_at(Rect::new(0, 0, 80, 30), DisplayPosition { row: 26, col: 4 });
        assert_eq!(info.area, Rect::new(4, 27, 6, 3));
    }

    #[test]
    fn review_keeps_full_content_height_for_scrolling() {
        use crate::ui::change_review::ReviewView;
        use view::features::change_review::Review;

        let review = Review::new("old\n".repeat(30).into(), "new\n".repeat(30).into(), None);
        let mut popup = Popup::new("test", ReviewView::new("hunk".into(), review));
        let info =
            popup.render_info_at(Rect::new(0, 0, 80, 40), DisplayPosition { row: 31, col: 4 });
        assert_eq!(info.area, Rect::new(0, 5, 78, MAX_HEIGHT));
        assert!(info.child_height > MAX_HEIGHT);
        assert!(info.render_borders);
    }

    #[test]
    fn placement_is_recomputed_after_resizing() {
        let mut popup = text_popup(16, Open::Below);
        let position = DisplayPosition { row: 22, col: 4 };
        for (height, expected_y) in [(50, 23), (30, 4), (50, 23)] {
            let info = popup.render_info_at(Rect::new(0, 0, 80, height), position);
            assert_eq!(info.area, Rect::new(4, expected_y, 6, 18));
        }
    }

    #[test]
    fn popup_stays_inside_small_and_offset_viewports() {
        for viewport in [
            Rect::new(0, 0, 0, 0),
            Rect::new(3, 5, 1, 1),
            Rect::new(3, 5, 3, 3),
            Rect::new(3, 5, 80, 30),
        ] {
            for position in [
                DisplayPosition { row: 0, col: 0 },
                DisplayPosition { row: 27, col: 80 },
                DisplayPosition {
                    row: usize::MAX,
                    col: usize::MAX,
                },
            ] {
                let area = text_popup(50, Open::Below)
                    .render_info_at(viewport, position)
                    .area;
                assert_eq!(area.intersection(viewport), area);
            }
        }
    }

    #[test]
    fn tall_popup_uses_space_above_instead_of_squeezing_below() {
        let viewport = Rect::new(0, 0, 80, 30);
        let mut popup = text_popup(16, Open::Below);
        let info = popup.render_info_at(viewport, DisplayPosition { row: 22, col: 4 });
        assert_eq!(info.area, Rect::new(4, 4, 6, 18));
    }

    #[test]
    fn oversized_popup_uses_the_larger_side_when_neither_side_fits() {
        let viewport = Rect::new(0, 0, 80, 24);
        let mut popup = text_popup(50, Open::Below);
        let info = popup.render_info_at(viewport, DisplayPosition { row: 16, col: 4 });
        assert_eq!(info.area, Rect::new(4, 0, 6, 16));
    }
}
