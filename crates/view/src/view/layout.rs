//! Geometry and formatted text shared by rendering, motion, and hit testing.

use super::ViewPosition;
use crate::graphics::Rect;
use editor_core::RopeSlice;
use stdx::rope::RopeExt as _;
use text_layout::{
    char_idx_at_visual_offset,
    doc_formatter::{DocumentFormatter, InlineAnnotationSource, TextFormat},
    text_annotations::TextAnnotations,
    visual_offset_from_anchor, visual_offset_from_block, DisplayPosition, VisualOffsetError,
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
    /// Exact inline text displayed at a screen cell. Padding from clipping a
    /// wide glyph is excluded; a visible tab still owns each of its cells.
    pub fn inline_annotation_at_screen_coords(
        &self,
        row: u16,
        column: u16,
    ) -> Option<InlineAnnotationSource<'_>> {
        if row < self.viewport.top()
            || row >= self.viewport.bottom()
            || column < self.viewport.left()
            || column >= self.viewport.right()
        {
            return None;
        }
        let first_row = visual_offset_from_block(
            self.text,
            self.offset.anchor,
            self.offset.anchor,
            &self.format,
            &self.annotations,
        )
        .0
        .row;
        let row = first_row + self.offset.vertical_offset + usize::from(row - self.viewport.y);
        let column = self.offset.horizontal_offset + usize::from(column - self.viewport.x);
        for grapheme in DocumentFormatter::new_at_prev_checkpoint(
            self.text,
            &self.format,
            &self.annotations,
            self.offset.anchor,
        ) {
            if grapheme.visual_pos.row > row {
                break;
            }
            if grapheme.visual_pos.row == row
                && grapheme.visual_pos.col <= column
                && column < grapheme.visual_pos.col + grapheme.width()
            {
                // Rendering suppresses a wide glyph that crosses either edge.
                // Tabs are rendered as individual cells and may be clipped.
                if !matches!(grapheme.raw, editor_core::graphemes::Grapheme::Tab { .. })
                    && (grapheme.visual_pos.col < self.offset.horizontal_offset
                        || grapheme.visual_pos.col + grapheme.width()
                            > self.offset.horizontal_offset + usize::from(self.viewport.width))
                {
                    return None;
                }
                return grapheme.inline_annotation;
            }
        }
        None
    }

    /// Source line that begins the display block containing this position.
    pub fn block_line_at_pos(&self, pos: usize) -> usize {
        self.text
            .char_to_line_idx(self.annotations.checkpoint_anchor(self.text, pos))
    }

    /// A whole-line upper bound for text visible in this viewport. Folding
    /// expands the document range; wrapping and virtual lines can only shrink it.
    pub fn visible_byte_range(&self) -> std::ops::Range<usize> {
        let anchor = self
            .annotations
            .checkpoint_anchor(self.text, self.offset.anchor.min(self.text.len_chars()));
        let first = self.text.char_to_line_idx(anchor);
        let last = self.last_document_line();
        let end = self
            .annotations
            .block_end(self.text, self.text.line_to_char_idx(last));
        let end_line = self.text.char_to_line_idx(end);
        self.text.line_to_byte_idx(first, stdx::rope::LINE_TYPE)
            ..self
                .text
                .line_to_byte_idx(end_line + 1, stdx::rope::LINE_TYPE)
    }

    /// Upper bound for the final visible document line, including an empty EOF line.
    pub fn last_document_line(&self) -> usize {
        let anchor = self
            .annotations
            .checkpoint_anchor(self.text, self.offset.anchor.min(self.text.len_chars()));
        let first = self.text.char_to_line_idx(anchor);
        let count = self
            .offset
            .vertical_offset
            .saturating_add(self.viewport.height as usize)
            .saturating_sub(1);
        text_layout::movement::document_line_at_offset(
            self.text,
            first,
            editor_core::movement::Direction::Forward,
            count,
            self.annotations.conceals(),
        )
    }

    /// Visible byte ranges with concealed text removed, for syntax queries that
    /// should not traverse hidden bodies on every frame.
    pub fn visible_byte_ranges(&self) -> Vec<std::ops::Range<usize>> {
        let range = self.visible_byte_range();
        let mut start = range.start;
        let mut ranges = Vec::new();
        for conceal in self.annotations.conceals() {
            let hidden_start = self.text.char_to_byte_idx(conceal.range.start);
            let hidden_end = self.text.char_to_byte_idx(conceal.range.end);
            if hidden_end <= start {
                continue;
            }
            if hidden_start >= range.end {
                break;
            }
            if start < hidden_start {
                ranges.push(start..hidden_start);
            }
            start = hidden_end.min(range.end);
        }
        if start < range.end {
            ranges.push(start..range.end);
        }
        ranges
    }

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
            .char_to_line_idx(self.offset.anchor.min(self.text.len_chars()));
        if self.annotations.conceals().is_empty()
            && first_line.saturating_add(self.viewport.height as usize)
                < self.text.len_lines(stdx::rope::LINE_TYPE)
        {
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

    #[test]
    fn inline_hits_follow_wrapped_rows_scroll_offsets_and_viewport_edges() {
        let text = Rope::from_str("before\na tail\nnext");
        let inline = [InlineAnnotation::new(8, "界e\u{301} long 😀 label")];
        let mut layout = plain_layout(&text, Rect::new(7, 9, 6, 2));
        layout.offset.anchor = 7;
        layout.offset.vertical_offset = 1;
        layout.format = TextFormat {
            soft_wrap: true,
            viewport_width: 6,
            max_wrap: 0,
            max_indent_retain: 0,
            ..Default::default()
        };
        layout.annotations.add_inline_annotations(&inline, None);
        let glyphs = DocumentFormatter::new_at_prev_checkpoint(
            layout.text,
            &layout.format,
            &layout.annotations,
            7,
        )
        .collect::<Vec<_>>();
        let mut hits = 0;
        for glyph in glyphs {
            let Some(source) = glyph.inline_annotation else {
                continue;
            };
            if !(1..3).contains(&glyph.visual_pos.row) {
                continue;
            }
            for col in glyph.visual_pos.col..glyph.visual_pos.col + glyph.width() {
                let hit = layout
                    .inline_annotation_at_screen_coords(
                        9 + (glyph.visual_pos.row - 1) as u16,
                        7 + col as u16,
                    )
                    .unwrap();
                assert!(std::ptr::eq(hit.annotation, &inline[0]));
                assert_eq!(hit.byte_offset, source.byte_offset);
                hits += 1;
            }
        }
        assert!(hits > 5);
        for (row, col) in [(8, 7), (9, 6), (9, 13), (11, 7)] {
            assert!(layout
                .inline_annotation_at_screen_coords(row, col)
                .is_none());
        }
    }

    #[test]
    fn inline_hits_exclude_clipped_wide_glyphs_and_include_visible_tab_cells() {
        let text = Rope::from_str("x");
        let inline = [InlineAnnotation::new(0, "界a😀b")];
        let mut layout = plain_layout(&text, Rect::new(5, 7, 4, 1));
        layout.annotations.add_inline_annotations(&inline, None);
        layout.offset.horizontal_offset = 1;
        assert!(layout.inline_annotation_at_screen_coords(7, 5).is_none());
        assert_eq!(
            layout
                .inline_annotation_at_screen_coords(7, 6)
                .unwrap()
                .byte_offset,
            3
        );
        for col in [7, 8] {
            assert_eq!(
                layout
                    .inline_annotation_at_screen_coords(7, col)
                    .unwrap()
                    .byte_offset,
                4
            );
        }
        layout.offset.horizontal_offset = 2;
        layout.viewport.width = 2;
        assert_eq!(
            layout
                .inline_annotation_at_screen_coords(7, 5)
                .unwrap()
                .byte_offset,
            3
        );
        assert!(layout.inline_annotation_at_screen_coords(7, 6).is_none());

        let tab = [InlineAnnotation::new(0, "A\tB")];
        let mut layout = plain_layout(&text, Rect::new(5, 7, 2, 1));
        layout.annotations.add_inline_annotations(&tab, None);
        layout.offset.horizontal_offset = 2;
        for col in [5, 6] {
            assert_eq!(
                layout
                    .inline_annotation_at_screen_coords(7, col)
                    .unwrap()
                    .byte_offset,
                1
            );
        }
    }
}
