use crate::keymap::component::PopupAction;
use crate::{
    commands::Open,
    compositor::{Callback, Component, Context, Event, EventResult},
    key,
    ui::panel,
};
use tui::buffer::BufferExt as _;
use tui::{buffer::Buffer as Surface, scrollbar, widgets::Widget};
use ui_core::keymap::{Binding, KeymapState, Lookup};

use text_layout::DisplayPosition;
use ui_core::input::{MouseEvent, MouseEventKind};
use view::{graphics::Rect, Editor};

const MIN_HEIGHT: u16 = 6;
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

        // -- make sure frame doesn't stick out of bounds
        let mut rel_x = position.col as u16;
        let mut rel_y = position.row as u16;

        // if there's a orientation preference, use that
        // if we're on the top part of the screen, do below
        // if we're on the bottom part, do above
        let can_put_below = viewport.height > rel_y + MIN_HEIGHT;
        let can_put_above = rel_y.checked_sub(MIN_HEIGHT).is_some();
        let final_pos = match self.position_bias {
            Open::Below => match can_put_below {
                true => Open::Below,
                false => Open::Above,
            },
            Open::Above => match can_put_above {
                true => Open::Above,
                false => Open::Below,
            },
        };

        // compute maximum space available for child
        let mut max_height = match final_pos {
            Open::Above => rel_y,
            Open::Below => viewport.height.saturating_sub(1 + rel_y),
        };
        max_height = max_height.min(MAX_HEIGHT);
        let mut max_width = viewport.width.saturating_sub(2).min(MAX_WIDTH);
        let render_borders = max_height > 3 && max_width > 3;
        if render_borders {
            max_width -= 2;
            max_height -= 2;
        }

        // compute required child size and reclamp
        let (mut width, child_height) = self
            .contents
            .required_size((max_width, max_height))
            .expect("Component needs required_size implemented in order to be embedded in a popup");

        width = width.min(MAX_WIDTH);
        let height = if render_borders {
            (child_height + 2).min(MAX_HEIGHT)
        } else {
            child_height.min(MAX_HEIGHT)
        };
        if render_borders {
            width += 2;
        }
        if viewport.width <= rel_x + width + 2 {
            rel_x = viewport.width.saturating_sub(width + 2);
            width = viewport.width.saturating_sub(rel_x + 2)
        }

        let area = match final_pos {
            Open::Above => {
                rel_y = rel_y.saturating_sub(height);
                Rect::new(rel_x, rel_y, width, position.row as u16 - rel_y)
            }
            Open::Below => {
                rel_y += 1;
                let y_max = viewport.bottom().min(height + rel_y);
                Rect::new(rel_x, rel_y, width, y_max - rel_y)
            }
        };
        RenderInfo {
            area,
            child_height,
            render_borders,
        }
    }

    fn handle_mouse_event(&mut self, event: &MouseEvent, cx: &mut Context) -> EventResult {
        let &MouseEvent {
            kind,
            column: x,
            row: y,
            ..
        } = event;
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
            _ => self.contents.handle_event(&Event::Mouse(*event), cx),
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
            Event::Mouse(event) => return self.handle_mouse_event(event, cx),
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
            let block = panel::bordered(&cx.editor.theme, cx.editor.config().border_style);
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
                area,
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
    use super::clamp_scroll;

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
}
