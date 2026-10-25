//! Shared vertical scrollbar rendering.

use crate::{
    buffer::Buffer,
    style::Style as RatatuiStyle,
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget},
};
use ui_core::graphics::{Color, Rect, Style};

/// Renders a vertical scrollbar at the right edge of `area`.
///
/// In a bordered component only the thumb is drawn, preserving the existing
/// border as the track. In a borderless component both track and thumb use a
/// right-half block. Heights and `position` are expressed in rendered rows.
pub fn render(
    surface: &mut Buffer,
    area: Rect,
    viewport_height: u16,
    content_height: usize,
    position: usize,
    bordered: bool,
    style: Style,
) {
    let thumb = style.fg.unwrap_or(Color::Reset);
    let track = style.bg.unwrap_or(Color::Reset);
    let symbol = if bordered { "▌" } else { "▐" };
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_symbol(symbol)
        .thumb_style(RatatuiStyle::default().fg(thumb))
        .track_symbol((!bordered).then_some(symbol))
        .track_style(RatatuiStyle::default().fg(track));
    let mut state = ScrollbarState::new(content_height)
        .position(position)
        .viewport_content_length(viewport_height as usize);
    scrollbar.render(area, surface, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bordered_scrollbar_preserves_its_track() {
        let area = Rect::new(0, 0, 5, 4);
        let mut surface = Buffer::with_lines(["....|", "....|", "....|", "....|"]);

        render(
            &mut surface,
            area,
            area.height,
            8,
            0,
            true,
            Style::default(),
        );

        let edge: Vec<_> = (0..area.height)
            .map(|y| surface[(area.right() - 1, y)].symbol())
            .collect();
        assert!(edge.contains(&"▌"));
        assert!(edge.contains(&"|"));
    }

    #[test]
    fn borderless_scrollbar_draws_its_track() {
        let area = Rect::new(0, 0, 5, 4);
        let mut surface = Buffer::empty(area);

        render(
            &mut surface,
            area,
            area.height,
            8,
            0,
            false,
            Style::default(),
        );

        assert!((0..area.height).all(|y| surface[(area.right() - 1, y)].symbol() == "▐"));
    }
}
