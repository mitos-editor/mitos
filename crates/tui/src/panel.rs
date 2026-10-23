//! Shared panel chrome and geometry.

use crate::{
    layout::Rect,
    symbols::border,
    widgets::{Block, Borders, Padding},
};
use ui_core::graphics::Style;

/// Padding, in terminal cells, applied by padded panel blocks.
pub const PADDING: u16 = 1;
/// Space consumed by left and right borders together.
pub const BORDER_INSET: u16 = 2;
/// Space consumed by horizontal padding on both sides.
pub const HORIZONTAL_INSET: u16 = PADDING * 2;
/// Space consumed by vertical padding on both sides.
pub const VERTICAL_INSET: u16 = PADDING * 2;
/// Total horizontal space consumed by a bordered, padded panel.
pub const PADDED_HORIZONTAL_INSET: u16 = BORDER_INSET + HORIZONTAL_INSET;
/// Total vertical space consumed by a bordered, padded panel.
pub const PADDED_VERTICAL_INSET: u16 = BORDER_INSET + VERTICAL_INSET;

/// Creates a block with borders on every side.
/// Use [`border::EMPTY`] to keep the border space without visible glyphs.
pub fn bordered(style: Style, border_set: border::Set<'static>) -> Block<'static> {
    Block::bordered().border_style(style).border_set(border_set)
}

/// Creates a bordered block with one cell of horizontal padding.
pub fn horizontally_padded(style: Style, border_set: border::Set<'static>) -> Block<'static> {
    bordered(style, border_set).padding(Padding::horizontal(PADDING))
}

/// Creates a bordered block with one cell of padding on every side.
pub fn uniformly_padded(style: Style, border_set: border::Set<'static>) -> Block<'static> {
    bordered(style, border_set).padding(Padding::uniform(PADDING))
}

/// Creates a separator block with only a top border.
pub fn top_border(style: Style, border_set: border::Set<'static>) -> Block<'static> {
    bordered(style, border_set).borders(Borders::TOP)
}

/// Returns the area remaining after one cell of padding on every side.
///
/// Ratatui saturates the dimensions of undersized areas at zero.
pub fn content_area(area: Rect) -> Rect {
    Block::new().padding(Padding::uniform(PADDING)).inner(area)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buffer::Buffer, widgets::Widget};

    #[test]
    fn panel_and_separator_use_the_selected_border_symbols() {
        for (border_set, corner, horizontal) in [
            (border::EMPTY, " ", " "),
            (border::PLAIN, "┌", "─"),
            (border::ROUNDED, "╭", "─"),
            (border::DOUBLE, "╔", "═"),
            (border::THICK, "┏", "━"),
        ] {
            let style = Style::default();
            let area = Rect::new(0, 0, 5, 3);
            let mut frame = Buffer::empty(area);
            bordered(style, border_set).render(area, &mut frame);
            assert_eq!(frame[(0, 0)].symbol(), corner);
            assert_eq!(frame[(1, 0)].symbol(), horizontal);

            let mut separator = Buffer::empty(area);
            top_border(style, border_set).render(area, &mut separator);
            assert_eq!(separator[(0, 0)].symbol(), horizontal);
            assert_eq!(separator[(1, 0)].symbol(), horizontal);
            assert_eq!(separator[(0, 1)].symbol(), " ");
            assert_eq!(separator[(0, 2)].symbol(), " ");
        }
    }

    #[test]
    fn content_area_applies_a_uniform_inset() {
        assert_eq!(Rect::new(4, 6, 8, 3), content_area(Rect::new(3, 5, 10, 5)));
    }

    #[test]
    fn content_area_saturates_for_tiny_panels() {
        assert_eq!(Rect::new(4, 6, 0, 0), content_area(Rect::new(3, 5, 1, 1)));
    }
}
