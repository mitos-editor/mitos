//! Shared clipping for labels after the last visual segment of a source line.

use crate::ui::document::{LinePos, TextRenderer};
use editor_core::unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr};
use text_layout::DisplayPosition;
use theme::Style;

pub(super) fn draw_label(
    renderer: &mut TextRenderer,
    pos: LinePos,
    off: DisplayPosition,
    labels: &str,
    style: Style,
) -> DisplayPosition {
    let start = off.col.saturating_add(1);
    let right = renderer.offset.col + renderer.viewport.width as usize;
    if start >= right {
        return DisplayPosition::new(0, 0);
    }
    let col = start.saturating_sub(renderer.offset.col);
    let width = renderer.viewport.width as usize - col;
    // Keep labels readable when only a few columns remain.
    if width < 4 {
        return DisplayPosition::new(0, 0);
    }
    let mut logical_col = start;
    let visible: String = labels
        .graphemes(true)
        .filter_map(|grapheme| {
            let from = logical_col;
            logical_col += grapheme.width();
            if from >= renderer.offset.col {
                Some(grapheme.to_owned())
            } else if logical_col > renderer.offset.col {
                Some(" ".repeat(logical_col - renderer.offset.col))
            } else {
                None
            }
        })
        .collect();
    if visible.is_empty() {
        // Later decorations still follow this label when it is entirely left of the viewport.
        return DisplayPosition::new(0, logical_col.saturating_sub(off.col));
    }
    let ellipsis = visible.width() > width;
    let (end, _) = renderer.set_string_truncated(
        renderer.viewport.x + col as u16,
        pos.visual_line,
        &visible,
        width,
        |_| style,
        ellipsis,
        false,
    );
    let logical_end = end.saturating_sub(renderer.viewport.x) as usize
        + renderer.offset.col
        + usize::from(ellipsis);
    DisplayPosition::new(0, logical_end.saturating_sub(off.col))
}
