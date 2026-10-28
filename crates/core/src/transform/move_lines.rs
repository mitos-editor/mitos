//! Line-content rotations and their explicit selection mapping.

use crate::{
    line_ending::line_end_char_index, Range, Rope, RopeSlice, Selection, SmallVec, Transaction,
};
use stdx::rope::RopeExt as _;

/// Which side of the selected lines receives the block.
#[derive(Debug, Clone, Copy)]
pub enum MoveLinesDirection {
    Above,
    Below,
}

/// Move adjacent selected lines together, preserving line endings and selection direction.
///
/// A count behaves like repeated single moves, composed into one transaction.
/// Blocks stop at document edges. A multiline block ending in an empty line also stops
/// before an unterminated final line: its empty tail cannot be selected at EOF
/// without changing which lines the next invocation moves.
pub fn move_lines(
    text: &Rope,
    selection: &Selection,
    direction: MoveLinesDirection,
    count: usize,
) -> Transaction {
    // No block can travel farther than the number of lines in the document.
    let count = count.min(text.len_lines(stdx::rope::LINE_TYPE).saturating_sub(1));
    let mut text = text.clone();
    let mut selection = selection.clone();
    let mut transaction = Transaction::new(&text).with_selection(selection.clone());
    for _ in 0..count {
        let step = move_once(&text, &selection, direction);
        let next_selection = step.selection().unwrap();
        if step.changes().is_empty() && next_selection == &selection {
            break;
        }
        step.apply(&mut text);
        // Match the selection invariants applied by Document after each keypress.
        selection = next_selection.clone().ensure_invariants(text.slice(..));
        transaction = transaction.compose(step.with_selection(selection.clone()));
    }
    transaction
}

fn line_content(text: RopeSlice<'_>, line: usize) -> RopeSlice<'_> {
    text.slice_chars(text.line_to_char_idx(line)..line_end_char_index(&text, line))
}

fn ending_len(text: RopeSlice<'_>, line: usize) -> usize {
    text.line(line, stdx::rope::LINE_TYPE).len_chars() - line_content(text, line).len_chars()
}

fn move_once(text: &Rope, selection: &Selection, direction: MoveLinesDirection) -> Transaction {
    let slice = text.slice(..);
    let total_lines = text.len_lines(stdx::rope::LINE_TYPE);
    let mut edits = Vec::new();
    let mut ranges = SmallVec::with_capacity(selection.len());
    let mut selected = selection.iter().peekable();

    for (start, end) in selection.line_ranges(slice) {
        let window = match direction {
            MoveLinesDirection::Above if start > 0 => Some((start - 1, end)),
            MoveLinesDirection::Below if end + 1 < total_lines => {
                let next = end + 1;
                let loses_empty_tail = start < end
                    && next == total_lines - 1
                    && line_content(slice, end).len_chars() == 0;
                (!loses_empty_tail).then_some((start, next))
            }
            _ => None,
        };

        if let Some((first, last)) = window {
            for line in first..=last {
                let source = match direction {
                    MoveLinesDirection::Below if line == first => last,
                    MoveLinesDirection::Below => line - 1,
                    MoveLinesDirection::Above if line == last => first,
                    MoveLinesDirection::Above => line + 1,
                };
                let content = line_content(slice, source);
                if content != line_content(slice, line) {
                    edits.push((
                        text.line_to_char_idx(line),
                        line_end_char_index(&slice, line),
                        Some(content.to_string().into()),
                    ));
                }
            }
        }

        // Each range belongs to exactly one merged block. Map its endpoints
        // with that block, including an endpoint just after its last newline.
        while let Some(range) = selected.next_if(|range| range.line_range(slice).0 <= end) {
            let Some((first, last)) = window else {
                ranges.push(*range);
                continue;
            };
            let move_pos = |pos| {
                let past_end = end + 1 < total_lines && pos == text.line_to_char_idx(end + 1);
                let line = slice.char_to_line_idx(pos);
                match direction {
                    MoveLinesDirection::Below => {
                        pos + line_content(slice, last).len_chars()
                            + ending_len(slice, if past_end { last } else { line })
                    }
                    MoveLinesDirection::Above => {
                        pos - line_content(slice, first).len_chars()
                            - ending_len(slice, if past_end { end } else { line - 1 })
                    }
                }
            };
            ranges.push(
                Range::new(move_pos(range.anchor), move_pos(range.head))
                    .with_direction(range.direction()),
            );
        }
    }

    Transaction::change(text, edits.into_iter())
        .with_selection(Selection::new(ranges, selection.primary_index()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test;

    fn moved(input: &str, direction: MoveLinesDirection, count: usize) -> String {
        let (text, selection) = test::print(input);
        let mut text = Rope::from(text);
        let transaction = move_lines(&text, &selection, direction, count);
        assert!(transaction.apply(&mut text));
        test::plain(text.slice(..), transaction.selection().unwrap())
    }

    #[test]
    fn counts_move_whole_blocks_and_stop_at_edges() {
        for (input, direction, count, expected) in [
            (
                "#[a|]#\nb\nc\nd",
                MoveLinesDirection::Below,
                2,
                "b\nc\n#[a|]#\nd",
            ),
            (
                "a\nb\nc\n#[|d]#",
                MoveLinesDirection::Above,
                2,
                "a\n#[|d]#\nb\nc",
            ),
            (
                "#[a|]#\nb\nc",
                MoveLinesDirection::Below,
                usize::MAX,
                "b\nc\n#[a|]#",
            ),
            (
                "a\nb\n#[c|]#",
                MoveLinesDirection::Above,
                usize::MAX,
                "#[c|]#\na\nb",
            ),
            ("#[a|]#\nb", MoveLinesDirection::Below, 0, "#[a|]#\nb"),
            ("#[|]#", MoveLinesDirection::Below, usize::MAX, "#[|]#"),
        ] {
            assert_eq!(moved(input, direction, count), expected);
        }
    }

    #[test]
    fn empty_block_tail_stops_before_unterminated_eof() {
        for input in ["#[a\n\n|]#b", "#[a\r\n\r\n|]#b"] {
            assert_eq!(moved(input, MoveLinesDirection::Below, usize::MAX), input);
        }
        assert_eq!(
            moved("#[a\n\n|]#b\nc", MoveLinesDirection::Below, usize::MAX),
            "b\n#[a\n\n|]#c",
        );
    }

    #[test]
    fn single_empty_lines_can_move_to_and_from_eof() {
        assert_eq!(moved("#[\n|]#b", MoveLinesDirection::Below, 1), "b\n#[|]#",);
        assert_eq!(moved("b\n#[|]#", MoveLinesDirection::Above, 1), "#[\n|]#b",);
    }

    #[test]
    fn unicode_offsets_direction_and_primary_survive_counted_moves() {
        assert_eq!(
            moved(
                "#(界|)#\r\n#[|e\u{301}]#\nlast\r\nend",
                MoveLinesDirection::Below,
                2
            ),
            "last\r\nend\n#(界|)#\r\n#[|e\u{301}]#",
        );
    }

    #[test]
    fn counted_moves_match_repeated_moves_with_merging_blocks() {
        for input in [
            "#[a|]#\nb\n#(c|)#\nd",
            "#[a|]#\naa\naa\naa",
            "a\n#[b\n\n|]#c\nd\n",
            "#(界|)#\r\n#[|e\u{301}]#\nlast\r\nend",
            "a\nb\n#[|]#",
        ] {
            for direction in [MoveLinesDirection::Above, MoveLinesDirection::Below] {
                let (original, original_selection) = test::print(input);
                let original = Rope::from(original);
                let mut text = original.clone();
                let mut selection = original_selection.clone();
                for count in 0..8 {
                    let combined = move_lines(&original, &original_selection, direction, count);
                    let mut counted_text = original.clone();
                    assert!(combined.apply(&mut counted_text));
                    assert_eq!(counted_text, text);
                    assert_eq!(combined.selection(), Some(&selection));
                    let inverse = combined.invert(&original);
                    assert!(inverse.apply(&mut counted_text));
                    assert_eq!(counted_text, original);

                    let step = move_lines(&text, &selection, direction, 1);
                    assert!(step.apply(&mut text));
                    selection = step.selection().unwrap().clone();
                }
            }
        }
    }
}
