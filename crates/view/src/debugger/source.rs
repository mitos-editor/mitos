//! DAP source locations use one-based lines and UTF-16 columns.

use dap::StackFrame;
use editor_core::{line_ending::get_line_ending, Rope, Selection};

/// Convert a DAP source line to an editor line, rejecting the unknown line zero.
pub fn line_index(line: usize) -> Option<usize> {
    line.checked_sub(1)
}

/// Convert a DAP position without allowing its column to cross a line ending.
/// Unknown columns (zero) use the line start; oversized columns use the line end.
/// A column inside a surrogate pair resolves to that character's start.
pub fn position_to_char(text: &Rope, line: usize, column: usize) -> Option<usize> {
    let line = line_index(line)?;
    let content = text.get_line(line)?;
    let end =
        content.len_chars() - get_line_ending(&content).map_or(0, |ending| ending.len_chars());
    let content = content.slice(..end);
    let column = column.saturating_sub(1).min(content.len_utf16_cu());
    Some(text.line_to_char(line) + content.utf16_cu_to_char(column))
}

/// A stack frame's inclusive line range for source previews and gutter markers.
pub fn frame_line_range(frame: &StackFrame) -> Option<(usize, usize)> {
    let start = line_index(frame.line)?;
    let end = frame
        .end_line
        .and_then(line_index)
        .unwrap_or(start)
        .max(start);
    Some((start, end))
}

pub(crate) fn frame_selection(text: &Rope, frame: &StackFrame) -> Option<Selection> {
    let start = position_to_char(text, frame.line, frame.column)?;
    let end = if frame.end_line.is_some() || frame.end_column.is_some() {
        position_to_char(
            text,
            frame.end_line.unwrap_or(frame.line),
            frame.end_column.unwrap_or(0),
        )
        .filter(|end| *end >= start)
        .unwrap_or(start)
    } else {
        start
    };
    Some(Selection::single(start, end))
}

impl crate::Editor {
    pub(crate) fn navigate_debugger_frame(&mut self, id: dap::DebugAdapterId, frame: &StackFrame) {
        if self
            .debugger
            .get_active_session()
            .is_some_and(|session| session.id() == id)
            && !self.debugger.is_terminating(id)
        {
            crate::handlers::dap::jump_to_stack_frame(self, frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_follow_utf16_and_remain_inside_lf_and_crlf_source_lines() {
        for ending in ["\n", "\r\n"] {
            let text = Rope::from_str(&format!("a😀b{ending}next"));
            for (column, character) in [
                (0, 0),
                (1, 0),
                (2, 1),
                (3, 1),
                (4, 2),
                (5, 3),
                (usize::MAX, 3),
            ] {
                assert_eq!(position_to_char(&text, 1, column), Some(character));
            }
            assert_eq!(
                position_to_char(&text, 2, 1),
                Some(3 + ending.chars().count())
            );
            for line in [0, 3, usize::MAX] {
                assert_eq!(position_to_char(&text, line, 1), None);
            }
        }
        assert_eq!(position_to_char(&Rope::new(), 1, usize::MAX), Some(0));
    }

    #[test]
    fn frame_ranges_support_same_line_end_columns_and_ignore_reversed_ends() {
        let text = Rope::from_str("a😀b");
        let mut frame: StackFrame = serde_json::from_value(
            serde_json::json!({"id":1,"name":"main","line":1,"column":2,"endColumn":4}),
        )
        .unwrap();
        assert_eq!(
            frame_selection(&text, &frame),
            Some(Selection::single(1, 2))
        );
        frame.end_column = Some(1);
        assert_eq!(frame_selection(&text, &frame), Some(Selection::point(1)));
        frame.line = 0;
        assert!(frame_selection(&text, &frame).is_none());
        assert!(frame_line_range(&frame).is_none());
    }
}
