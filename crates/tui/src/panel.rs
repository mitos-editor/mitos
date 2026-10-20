//! Shared panel chrome and geometry.

use crate::{
    layout::Rect,
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
pub fn bordered(style: Style) -> Block<'static> {
    let mut block = Block::bordered().border_style(style);
    if let Some(border_type) = style.border_type {
        block = block.border_type(border_type);
    }

    block
}

/// Creates a bordered block with one cell of horizontal padding.
pub fn horizontally_padded(style: Style) -> Block<'static> {
    bordered(style).padding(Padding::horizontal(PADDING))
}

/// Creates a bordered block with one cell of padding on every side.
pub fn uniformly_padded(style: Style) -> Block<'static> {
    bordered(style).padding(Padding::uniform(PADDING))
}

/// Creates a separator block with only a top border.
pub fn top_border(style: Style) -> Block<'static> {
    Block::new().borders(Borders::TOP).border_style(style)
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

    #[test]
    fn content_area_applies_a_uniform_inset() {
        assert_eq!(Rect::new(4, 6, 8, 3), content_area(Rect::new(3, 5, 10, 5)));
    }

    #[test]
    fn content_area_saturates_for_tiny_panels() {
        assert_eq!(Rect::new(4, 6, 0, 0), content_area(Rect::new(3, 5, 1, 1)));
    }
}
