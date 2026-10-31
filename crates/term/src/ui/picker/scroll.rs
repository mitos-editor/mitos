//! Half-page scrolling shared by file, directory, and Git previews.

use tui::buffer::Buffer;
use view::graphics::Rect;

#[derive(Default)]
pub(crate) struct PreviewScroll {
    pub offset: usize,
    height: u16,
    content_height: usize,
}

impl PreviewScroll {
    pub fn update(&mut self, height: u16, content_height: usize) {
        self.height = height;
        self.content_height = content_height;
        self.offset = self
            .offset
            .min(content_height.saturating_sub(height as usize));
    }

    pub fn scroll(&mut self, down: bool) {
        let step = usize::from((self.height / 2).max(1));
        self.offset = if down {
            self.offset
                .saturating_add(step)
                .min(self.content_height.saturating_sub(self.height as usize))
        } else {
            self.offset.saturating_sub(step)
        };
    }

    pub fn render(&self, area: Rect, surface: &mut Buffer, theme: &theme::Theme) {
        if !area.is_empty() && self.content_height > self.height as usize {
            tui::scrollbar::render(
                surface,
                area,
                self.height,
                self.content_height,
                self.offset,
                false,
                theme.get("ui.menu.scroll"),
            );
        }
    }
}

/// Keep the scrollbar clear of text, even when a resize changes the overflow.
pub(crate) fn text_area(area: Rect) -> Rect {
    Rect {
        width: area.width.saturating_sub(1),
        ..area
    }
}
