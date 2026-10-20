//! Character and logical line-ending searches in document coordinates.

use super::{Direction, Movement};
use crate::{
    graphemes::{ensure_grapheme_boundary_prev, next_grapheme_boundary, prev_grapheme_boundary},
    Range, RopeSlice,
};
use stdx::rope::RopeExt as _;

/// Find a character in either direction, optionally stopping just before it.
/// Repeated till motions skip the adjacent grapheme so they can advance again.
/// Missing matches and a zero count leave the range unchanged.
pub fn find_char(
    text: RopeSlice,
    range: Range,
    ch: char,
    count: usize,
    direction: Direction,
    inclusive: bool,
    behaviour: Movement,
) -> Range {
    let cursor = range.cursor(text);
    let next = next_grapheme_boundary(text, cursor);
    let start = match (inclusive, direction) {
        (true, Direction::Forward) => next,
        (true, Direction::Backward) => cursor,
        (false, Direction::Forward) => next_grapheme_boundary(text, next),
        (false, Direction::Backward) => prev_grapheme_boundary(text, cursor),
    };
    let Some(pos) = crate::search::find_nth_char(count, text, ch, start, direction) else {
        return range;
    };
    let pos = ensure_grapheme_boundary_prev(text, pos);
    let pos = match (inclusive, direction) {
        (true, _) => pos,
        (false, Direction::Forward) => prev_grapheme_boundary(text, pos),
        (false, Direction::Backward) => next_grapheme_boundary(text, pos),
    };
    found_position(text, range, pos, behaviour)
}

/// Find logical line endings, including documents that mix LF and CRLF.
/// Till motions stop at the neighboring grapheme; a missing ending leaves the range unchanged.
pub fn find_line_ending(
    text: RopeSlice,
    range: Range,
    count: usize,
    direction: Direction,
    inclusive: bool,
    behaviour: Movement,
) -> Range {
    use crate::line_ending::line_end_char_index;
    if count == 0 {
        return range;
    }
    let cursor = range.cursor(text);
    let cursor_line = range.cursor_line(text);
    let pos = match direction {
        Direction::Forward => {
            let end = line_end_char_index(&text, cursor_line);
            let on_edge =
                end == cursor || (!inclusive && end == next_grapheme_boundary(text, cursor));
            let line = cursor_line
                .saturating_add(count - 1)
                .saturating_add(usize::from(on_edge));
            if line >= text.len_lines(stdx::rope::LINE_TYPE) - 1 {
                return range;
            }
            let end = line_end_char_index(&text, line);
            if inclusive {
                end
            } else {
                prev_grapheme_boundary(text, end)
            }
        }
        Direction::Backward => {
            let skip_current = inclusive || text.line_to_char_idx(cursor_line) == cursor;
            let Some(line) = cursor_line.checked_sub(count - usize::from(!skip_current)) else {
                return range;
            };
            if inclusive {
                line_end_char_index(&text, line)
            } else if line > 0 {
                text.line_to_char_idx(line)
            } else {
                return range;
            }
        }
    };
    found_position(text, range, pos, behaviour)
}

fn found_position(text: RopeSlice, range: Range, pos: usize, behaviour: Movement) -> Range {
    let origin = if behaviour == Movement::Extend {
        range
    } else {
        Range::point(range.cursor(text))
    };
    origin.put_cursor(text, pos, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test, Rope};

    #[test]
    fn matching_a_combining_character_targets_its_whole_grapheme() {
        let text = Rope::from("a e\u{301} x\n");
        let text = text.slice(..);
        let start = Range::new(0, 1);
        let found = find_char(
            text,
            start,
            '\u{301}',
            1,
            Direction::Forward,
            true,
            Movement::Move,
        );
        assert_eq!(found.cursor(text), 2);
        assert_eq!(found, found.grapheme_aligned(text));
        let till = find_char(
            text,
            start,
            '\u{301}',
            1,
            Direction::Forward,
            false,
            Movement::Move,
        );
        assert_eq!(till.cursor(text), 1);
        let backward = find_char(
            text,
            Range::new(5, 6),
            '\u{301}',
            1,
            Direction::Backward,
            false,
            Movement::Move,
        );
        assert_eq!(backward.cursor(text), 4);
    }

    #[test]
    fn find_and_till_respect_graphemes_directions_and_repeats() {
        let text = Rope::from("a👩‍💻x éx 界x\n");
        let text = text.slice(..);
        let range = Range::new(0, 1);
        let till = find_char(
            text,
            range,
            'x',
            1,
            Direction::Forward,
            false,
            Movement::Move,
        );
        assert_eq!(till.cursor(text), 1);
        let repeated = find_char(
            text,
            till,
            'x',
            1,
            Direction::Forward,
            false,
            Movement::Move,
        );
        assert_eq!(repeated.cursor(text), 6);
        let backward = find_char(
            text,
            Range::new(11, 12),
            'x',
            1,
            Direction::Backward,
            false,
            Movement::Move,
        );
        assert_eq!(backward.cursor(text), 9);
        let extended = find_char(
            text,
            Range::new(1, 0),
            '界',
            1,
            Direction::Forward,
            true,
            Movement::Extend,
        );
        assert_eq!(extended.from(), 0);
        assert_eq!(extended.cursor(text), 10);
        assert_eq!(
            find_char(
                text,
                range,
                'x',
                0,
                Direction::Forward,
                true,
                Movement::Move
            ),
            range
        );
        assert_eq!(
            find_char(
                text,
                range,
                'z',
                1,
                Direction::Forward,
                true,
                Movement::Move
            ),
            range
        );
        assert_eq!(
            find_char(
                text,
                range,
                'x',
                usize::MAX,
                Direction::Forward,
                true,
                Movement::Move
            ),
            range
        );
    }

    #[test]
    fn line_ending_search_handles_mixed_endings_empty_lines_and_document_edges() {
        let text = Rope::from("a👩‍💻\r\n\n界é\nlast");
        let text = text.slice(..);
        let range = Range::new(0, 1);
        assert_eq!(
            find_line_ending(text, range, 1, Direction::Forward, true, Movement::Move).cursor(text),
            4
        );
        assert_eq!(
            find_line_ending(text, range, 1, Direction::Forward, false, Movement::Move)
                .cursor(text),
            1
        );
        assert_eq!(
            find_line_ending(text, range, 2, Direction::Forward, true, Movement::Move).cursor(text),
            6
        );
        let last = Range::new(11, 12);
        assert_eq!(
            find_line_ending(text, last, 1, Direction::Backward, true, Movement::Move).cursor(text),
            10
        );
        assert_eq!(
            find_line_ending(text, last, 1, Direction::Backward, false, Movement::Move)
                .cursor(text),
            7
        );
        for direction in [Direction::Forward, Direction::Backward] {
            for inclusive in [true, false] {
                for count in [0, usize::MAX] {
                    assert_eq!(
                        find_line_ending(text, range, count, direction, inclusive, Movement::Move),
                        range
                    );
                }
            }
        }
    }

    #[test]
    fn searches_preserve_multicursor_primary_and_extension_anchors() {
        let (source, selection) = test::print("#(a|)# x #[b|]# x\n");
        let text = Rope::from(source);
        let found = selection.clone().transform(|range| {
            find_char(
                text.slice(..),
                range,
                'x',
                1,
                Direction::Forward,
                true,
                Movement::Extend,
            )
        });
        assert_eq!(found.primary_index(), selection.primary_index());
        for (before, after) in selection.iter().zip(found.iter()) {
            assert_eq!(after.anchor, before.anchor);
            assert_eq!(text.char_at(after.cursor(text.slice(..))), 'x');
        }
    }
}
