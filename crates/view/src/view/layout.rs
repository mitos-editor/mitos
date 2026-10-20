//! Geometry and formatted text shared by rendering, motion, and hit testing.

use super::ViewPosition;
use crate::graphics::Rect;
use editor_core::RopeSlice;
use text_layout::{
    char_idx_at_visual_offset, doc_formatter::TextFormat, text_annotations::TextAnnotations,
    visual_offset_from_anchor, DisplayPosition, VisualOffsetError,
};

/// An operation-local layout. The document remains borrowed while its annotations are used.
/// Callers may supply explicit inputs for previews without creating a document/view binding.
pub struct ViewLayout<'a> {
    pub text: RopeSlice<'a>,
    pub viewport: Rect,
    pub offset: ViewPosition,
    pub format: TextFormat,
    pub annotations: TextAnnotations<'a>,
}

impl ViewLayout<'_> {
    /// Translate to a viewport-relative display position, rejecting positions outside the viewport.
    pub fn display_position_at_pos(&self, pos: usize) -> Option<DisplayPosition> {
        let mut pos = visual_offset_from_anchor(
            self.text,
            self.offset.anchor,
            pos,
            &self.format,
            &self.annotations,
            self.offset
                .vertical_offset
                .saturating_add(self.viewport.height as usize),
        )
        .ok()?
        .0;
        pos.row = pos.row.checked_sub(self.offset.vertical_offset)?;
        pos.col = pos.col.checked_sub(self.offset.horizontal_offset)?;
        (pos.row < self.viewport.height as usize && pos.col < self.viewport.width as usize)
            .then_some(pos)
    }

    /// Translate an absolute screen coordinate inside the text viewport.
    pub fn position_at_screen_coords(
        &self,
        row: u16,
        column: u16,
        ignore_virtual_text: bool,
    ) -> Option<usize> {
        if row < self.viewport.top()
            || row >= self.viewport.bottom()
            || column < self.viewport.left()
            || column >= self.viewport.right()
        {
            return None;
        }
        self.position_at_view_coords(
            row - self.viewport.y,
            column - self.viewport.x,
            ignore_virtual_text,
        )
    }

    /// Translate coordinates relative to the text viewport. Rows may extend past its bottom.
    pub fn position_at_view_coords(
        &self,
        row: u16,
        column: u16,
        ignore_virtual_text: bool,
    ) -> Option<usize> {
        let (position, virtual_lines) = char_idx_at_visual_offset(
            self.text,
            self.offset.anchor,
            (row as usize + self.offset.vertical_offset) as isize,
            column as usize + self.offset.horizontal_offset,
            &self.format,
            &self.annotations,
        );
        (virtual_lines == 0 || !ignore_virtual_text).then_some(position)
    }

    /// Last visible visual row relative to the viewport, clamped to the end of the text.
    pub fn last_visual_line(&self) -> usize {
        let height = self.offset.vertical_offset + self.viewport.height as usize;
        let first_line = self
            .text
            .char_to_line(self.offset.anchor.min(self.text.len_chars()));
        if first_line.saturating_add(self.viewport.height as usize) < self.text.len_lines() {
            return self.viewport.height.saturating_sub(1) as usize;
        }
        match visual_offset_from_anchor(
            self.text,
            self.offset.anchor,
            usize::MAX,
            &self.format,
            &self.annotations,
            height,
        ) {
            Ok((position, _)) => position.row.saturating_sub(self.offset.vertical_offset),
            Err(VisualOffsetError::PosAfterMaxRow) => {
                self.viewport.height.saturating_sub(1) as usize
            }
            Err(VisualOffsetError::PosBeforeAnchorRow) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::Rope;
    use text_layout::text_annotations::InlineAnnotation;

    fn plain_layout(text: &Rope, viewport: Rect) -> ViewLayout<'_> {
        ViewLayout {
            text: text.slice(..),
            viewport,
            offset: ViewPosition::default(),
            format: TextFormat::default(),
            annotations: TextAnnotations::default(),
        }
    }

    #[test]
    fn screen_positions_exclude_gutters_and_the_right_and_bottom_edges() {
        let text = Rope::from_str("abc\ndef");
        let layout = plain_layout(&text, Rect::new(7, 9, 3, 2));
        assert_eq!(layout.position_at_screen_coords(9, 7, true), Some(0));
        assert_eq!(layout.position_at_screen_coords(10, 9, true), Some(6));
        for (row, col) in [(8, 7), (9, 6), (9, 10), (11, 7)] {
            assert_eq!(layout.position_at_screen_coords(row, col, true), None);
        }
        let empty = plain_layout(&text, Rect::new(7, 9, 0, 0));
        assert_eq!(empty.position_at_screen_coords(9, 7, true), None);
        assert_eq!(empty.display_position_at_pos(0), None);
    }

    #[test]
    fn wrapped_positions_account_for_the_scrolled_rows_inside_a_document_line() {
        let text = Rope::from_str("abcdefghijklmnopqrstuvwxyz");
        let mut layout = plain_layout(&text, Rect::new(3, 5, 6, 2));
        layout.format.soft_wrap = true;
        layout.format.viewport_width = 6;
        layout.format.max_wrap = 0;
        layout.format.max_indent_retain = 0;
        layout.offset.vertical_offset = 2;
        assert_eq!(
            layout.display_position_at_pos(12),
            Some(DisplayPosition::new(0, 0))
        );
        assert_eq!(
            layout.display_position_at_pos(18),
            Some(DisplayPosition::new(1, 0))
        );
        assert_eq!(layout.position_at_screen_coords(6, 3, true), Some(18));
        assert_eq!(layout.display_position_at_pos(6), None);
        assert_eq!(layout.display_position_at_pos(24), None);
        assert_eq!(layout.last_visual_line(), 1);
    }

    #[test]
    fn document_positions_round_trip_with_unicode_tabs_and_inline_annotations() {
        let text = Rope::from_str("é\t界e\u{301}x");
        let inline = [InlineAnnotation::new(0, "hint")];
        let mut layout = plain_layout(&text, Rect::new(3, 5, 30, 2));
        layout.annotations.add_inline_annotations(&inline, None);
        for position in [0, 1, 2, 3, 5] {
            let display = layout.display_position_at_pos(position).unwrap();
            assert_eq!(
                layout.position_at_screen_coords(
                    5 + display.row as u16,
                    3 + display.col as u16,
                    true,
                ),
                Some(position)
            );
        }
        layout.offset.horizontal_offset = 4;
        assert_eq!(
            layout.display_position_at_pos(2),
            Some(DisplayPosition::new(0, 4))
        );
        assert_eq!(
            layout.display_position_at_pos(0),
            Some(DisplayPosition::new(0, 0))
        );
    }
}
