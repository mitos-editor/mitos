use text_layout::DisplayPosition;
use text_layout::{
    doc_formatter::{FormattedGrapheme, TextFormat},
    text_annotations::LineAnnotation,
};

/// Track and position preview rows for both line reservation and painting.
#[derive(Clone)]
pub struct InlineCompletionLines<'a> {
    cursor: usize,
    lines: &'a [String],
    format: TextFormat,
    line_end: usize,
    anchor: Option<DisplayPosition>,
    rows: Vec<InlineCompletionRow>,
    end_row: Option<usize>,
    reserved: bool,
}

impl<'a> InlineCompletionLines<'a> {
    pub fn new(cursor: usize, lines: &'a [String], format: TextFormat, line_end: usize) -> Self {
        Self {
            cursor,
            lines,
            format,
            line_end,
            anchor: None,
            rows: Vec::new(),
            end_row: None,
            reserved: false,
        }
    }

    pub fn reset_pos(&mut self, pos: usize) -> usize {
        self.anchor = None;
        self.rows.clear();
        self.end_row = None;
        self.reserved = false;
        if pos <= self.cursor {
            self.cursor
        } else {
            usize::MAX
        }
    }
    pub fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        if grapheme.is_virtual() {
            return grapheme.char_idx;
        }
        if grapheme.char_idx == self.cursor {
            self.anchor = Some(grapheme.visual_pos);
            self.rows = layout(self.lines, &self.format, grapheme.visual_pos.col);
        }
        if grapheme.char_idx >= self.line_end {
            self.end_row = Some(grapheme.visual_pos.row);
            usize::MAX
        } else {
            self.line_end
        }
    }
    /// Position the row over the source text and any remaining rows after its end.
    /// `offset` includes the rows reserved by earlier decorations on this line.
    pub fn block(
        &mut self,
        row: usize,
        offset: DisplayPosition,
    ) -> Option<InlineCompletionBlock<'_>> {
        let anchor = self.anchor?;
        if self.reserved {
            return None;
        }
        let relative = row.saturating_sub(anchor.row);
        let remaining = if self.end_row == Some(row) {
            self.reserved = true;
            &self.rows[(relative + 1).min(self.rows.len())..]
        } else {
            &[][..]
        };
        Some(InlineCompletionBlock {
            first: self.rows.get(relative),
            first_pos: DisplayPosition::new(row, if relative == 0 { anchor.col } else { 0 }),
            remaining,
            remaining_row: row + offset.row,
            soft_wrap: self.format.soft_wrap,
        })
    }
}

/// Borrowed, positioned preview rows. The first row also clears the covered source text
/// when the preview has fewer rows than the source line.
pub struct InlineCompletionBlock<'a> {
    first: Option<&'a InlineCompletionRow>,
    first_pos: DisplayPosition,
    remaining: &'a [InlineCompletionRow],
    remaining_row: usize,
    pub soft_wrap: bool,
}

impl InlineCompletionBlock<'_> {
    pub fn reserved_rows(&self) -> usize {
        self.remaining.len()
    }

    pub fn rows(&self) -> impl Iterator<Item = (DisplayPosition, Option<&InlineCompletionRow>)> {
        std::iter::once((self.first_pos, self.first)).chain(
            self.remaining
                .iter()
                .enumerate()
                .map(|(i, row)| (DisplayPosition::new(self.remaining_row + i, 0), Some(row))),
        )
    }
}

impl LineAnnotation for InlineCompletionLines<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.reset_pos(pos)
    }

    fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        self.process_anchor(grapheme)
    }

    fn insert_virtual_lines(
        &mut self,
        _: usize,
        end: DisplayPosition,
        _: usize,
    ) -> DisplayPosition {
        let rows = self
            .block(end.row, DisplayPosition::new(1, 0))
            .map_or(0, |block| block.reserved_rows());
        DisplayPosition::new(rows, 0)
    }
}

/// A visual preview row, produced by the same formatter used for document text.
#[derive(Clone)]
pub struct InlineCompletionRow {
    pub col: usize,
    pub text: String,
}

/// Format preview lines from an anchor column using the document text formatter.
pub fn layout(
    lines: &[String],
    format: &text_layout::doc_formatter::TextFormat,
    col: usize,
) -> Vec<InlineCompletionRow> {
    use editor_core::Rope;
    use text_layout::{doc_formatter::DocumentFormatter, text_annotations::TextAnnotations};
    // Padding supplies the actual anchor column to the formatter for tab stops and wrapping.
    // A non-whitespace sentinel prevents this padding from becoming retained indentation.
    let prefix = if col == 0 {
        String::new()
    } else {
        format!("x{}", " ".repeat(col - 1))
    };
    let text = Rope::from_str(&format!("{prefix}{}", lines.join("\n")));
    let annotations = TextAnnotations::default();
    let formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.slice(..), format, &annotations, 0);
    let mut rows = Vec::new();
    for grapheme in formatter {
        if grapheme.char_idx < col {
            continue;
        }
        while rows.len() <= grapheme.visual_pos.row {
            rows.push(InlineCompletionRow {
                col: 0,
                text: String::new(),
            });
        }
        let row = &mut rows[grapheme.visual_pos.row];
        if row.text.is_empty() {
            row.col = grapheme.visual_pos.col;
        }
        if !grapheme.source.is_eof()
            && !matches!(grapheme.raw, editor_core::graphemes::Grapheme::Newline)
        {
            row.text.push_str(&grapheme.raw.to_string());
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_rows_expand_tabs_at_the_anchor_and_preserve_unicode() {
        let rows = layout(
            &["\t界e\u{301}".into(), "".into()],
            &TextFormat::default(),
            2,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].col, rows[0].text.as_str()), (2, "  界e\u{301}"));
        assert_eq!((rows[1].col, rows[1].text.as_str()), (0, ""));
    }

    #[test]
    fn preview_rows_wrap_from_the_document_anchor_column() {
        let format = TextFormat {
            soft_wrap: true,
            viewport_width: 6,
            max_wrap: 0,
            max_indent_retain: 0,
            ..TextFormat::default()
        };
        let rows = layout(&["abcd界x".into()], &format, 2);
        let rows: Vec<_> = rows
            .iter()
            .map(|row| (row.col, row.text.as_str()))
            .collect();
        assert_eq!(rows, [(2, "abcd"), (0, "界x")]);
    }
}
