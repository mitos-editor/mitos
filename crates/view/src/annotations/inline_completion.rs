use crate::document::InlineCompletion;
use editor_core::{
    doc_formatter::{FormattedGrapheme, TextFormat},
    text_annotations::LineAnnotation,
    Position,
};

/// Reserve only the preview rows that don't fit in the original document line.
pub struct InlineCompletionLines<'a> {
    completion: &'a InlineCompletion,
    format: TextFormat,
    line_end: usize,
    anchor: Option<Position>,
    height: usize,
    at_end: bool,
    reserved: bool,
}

impl<'a> InlineCompletionLines<'a> {
    pub fn new(completion: &'a InlineCompletion, format: TextFormat, line_end: usize) -> Self {
        Self {
            completion,
            format,
            line_end,
            anchor: None,
            height: 0,
            at_end: false,
            reserved: false,
        }
    }
}

impl LineAnnotation for InlineCompletionLines<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.anchor = None;
        self.at_end = false;
        self.reserved = false;
        if pos <= self.completion.cursor {
            self.completion.cursor
        } else {
            usize::MAX
        }
    }
    fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        if grapheme.is_virtual() {
            return grapheme.char_idx;
        }
        if grapheme.char_idx == self.completion.cursor {
            self.anchor = Some(grapheme.visual_pos);
            self.height = self
                .completion
                .layout(&self.format, grapheme.visual_pos.col)
                .len();
        }
        self.at_end = grapheme.char_idx >= self.line_end;
        if self.at_end {
            usize::MAX
        } else {
            self.line_end
        }
    }
    fn insert_virtual_lines(&mut self, _: usize, end: Position, _: usize) -> Position {
        let Some(anchor) = self.anchor else {
            return Position::new(0, 0);
        };
        if !self.at_end || self.reserved {
            return Position::new(0, 0);
        }
        self.reserved = true;
        let existing_rows = end.row.saturating_sub(anchor.row) + 1;
        Position::new(self.height.saturating_sub(existing_rows), 0)
    }
}
