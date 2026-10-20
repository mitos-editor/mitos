//! Pure selection transformations in document coordinates.

use crate::{
    movement::{self, Direction},
    Position, Range, RopeSlice, Selection, SmallVec,
};
use std::cmp::Ordering;
use stdx::rope::RopeExt as _;

/// Trim surrounding whitespace, dropping empty ranges. If none remain,
/// retain the primary cursor so a selection is always nonempty.
pub fn trim(text: RopeSlice, selection: &Selection) -> Selection {
    let ranges: SmallVec<[Range; 1]> = selection
        .iter()
        .filter_map(|range| {
            if range.is_empty() || range.slice(text).chars().all(|ch| ch.is_whitespace()) {
                return None;
            }
            let mut start = range.from();
            let mut end = range.to();
            start = movement::skip_while(text, start, |x| x.is_whitespace()).unwrap_or(start);
            end = movement::backwards_skip_while(text, end, |x| x.is_whitespace()).unwrap_or(end);
            Some(Range::new(start, end).with_direction(range.direction()))
        })
        .collect();

    if !ranges.is_empty() {
        let primary = selection.primary();
        let idx = ranges
            .iter()
            .position(|range| range.overlaps(&primary))
            .unwrap_or(ranges.len() - 1);
        Selection::new(ranges, idx)
    } else {
        let cursor = selection.primary().cursor(text);
        Selection::single(cursor, cursor)
    }
}

/// Copy selections to document lines at the same cell columns. Skip lines that
/// are too short; soft wrapping and virtual text do not affect the columns.
/// The last copy of the primary range becomes primary.
pub fn copy_on_line(
    text: RopeSlice,
    selection: &Selection,
    tab_width: usize,
    direction: Direction,
    count: usize,
) -> Selection {
    use crate::{line_cell_coords_at_pos, pos_at_line_cell_coords};
    let mut ranges = SmallVec::with_capacity(selection.len());
    let mut primary_index = 0;
    for range in selection.iter() {
        let is_primary = *range == selection.primary();

        // The range is always head exclusive
        let (head, anchor) = if range.anchor < range.head {
            (range.head - 1, range.anchor)
        } else {
            (range.head, range.anchor.saturating_sub(1))
        };

        let head_pos = line_cell_coords_at_pos(text, head, tab_width);
        let anchor_pos = line_cell_coords_at_pos(text, anchor, tab_width);

        let height = std::cmp::max(head_pos.row, anchor_pos.row)
            - std::cmp::min(head_pos.row, anchor_pos.row)
            + 1;

        if is_primary {
            primary_index = ranges.len();
        }
        ranges.push(*range);

        let mut sels = 0;
        let mut i = 0;
        while sels < count {
            let offset = (i + 1) * height;

            let anchor_row = match direction {
                Direction::Forward => anchor_pos.row + offset,
                Direction::Backward => anchor_pos.row.saturating_sub(offset),
            };

            let head_row = match direction {
                Direction::Forward => head_pos.row + offset,
                Direction::Backward => head_pos.row.saturating_sub(offset),
            };

            if anchor_row >= text.len_lines(stdx::rope::LINE_TYPE)
                || head_row >= text.len_lines(stdx::rope::LINE_TYPE)
            {
                break;
            }

            let anchor =
                pos_at_line_cell_coords(text, Position::new(anchor_row, anchor_pos.col), tab_width);
            let head =
                pos_at_line_cell_coords(text, Position::new(head_row, head_pos.col), tab_width);

            // skip lines that are too short
            if line_cell_coords_at_pos(text, anchor, tab_width).col == anchor_pos.col
                && line_cell_coords_at_pos(text, head, tab_width).col == head_pos.col
            {
                if is_primary {
                    primary_index = ranges.len();
                }
                // This is Range::new(anchor, head), but it will place the cursor on the correct column
                ranges.push(Range::point(anchor).put_cursor(text, head, true));
                sels += 1;
            }

            if anchor_row == 0 && head_row == 0 {
                break;
            }

            i += 1;
        }
    }

    Selection::new(ranges, primary_index)
}

/// Extend to whole lines, continuing in the given direction when already aligned.
pub fn extend_line(
    text: RopeSlice,
    selection: &Selection,
    direction: Direction,
    count: usize,
) -> Selection {
    if count == 0 {
        return selection.clone();
    }
    selection.clone().transform(|range| {
        let (start_line, end_line) = range.line_range(text);

        let start = text.line_to_char_idx(start_line);
        let end = text.line_to_char_idx(
            (end_line + 1) // newline of end_line
                .min(text.len_lines(stdx::rope::LINE_TYPE)),
        );

        // extend to previous/next line if current line is selected
        let (anchor, head) = if range.from() == start && range.to() == end {
            match direction {
                Direction::Backward => {
                    (end, text.line_to_char_idx(start_line.saturating_sub(count)))
                }
                Direction::Forward => (
                    start,
                    text.line_to_char_idx(
                        (end_line + count + 1).min(text.len_lines(stdx::rope::LINE_TYPE)),
                    ),
                ),
            }
        } else {
            match direction {
                Direction::Backward => (
                    end,
                    text.line_to_char_idx(start_line.saturating_sub(count - 1)),
                ),
                Direction::Forward => (
                    start,
                    text.line_to_char_idx(
                        (end_line + count).min(text.len_lines(stdx::rope::LINE_TYPE)),
                    ),
                ),
            }
        };

        Range::new(anchor, head)
    })
}
/// Move the selection head by whole document lines.
pub fn select_line(
    text: RopeSlice,
    selection: &Selection,
    direction: Direction,
    count: usize,
) -> Selection {
    let saturating_add = |a: usize, b: usize| (a + b).min(text.len_lines(stdx::rope::LINE_TYPE));
    if count == 0 {
        return selection.clone();
    }
    selection.clone().transform(|range| {
        let (start_line, end_line) = range.line_range(text);
        let start = text.line_to_char_idx(start_line);
        let end = text.line_to_char_idx(saturating_add(end_line, 1));

        // Extending to line bounds is counted as one step for each range.
        let count = if range.from() != start || range.to() != end {
            count.saturating_sub(1)
        } else {
            count
        };
        let (anchor_line, head_line) = match (direction, range.direction()) {
            (Direction::Backward, Direction::Forward) => {
                (start_line, end_line.saturating_sub(count))
            }
            (Direction::Backward, Direction::Backward) => {
                (end_line, start_line.saturating_sub(count))
            }
            (Direction::Forward, Direction::Forward) => {
                (start_line, saturating_add(end_line, count))
            }
            (Direction::Forward, Direction::Backward) => {
                (end_line, saturating_add(start_line, count))
            }
        };
        let (anchor, head) = match anchor_line.cmp(&head_line) {
            Ordering::Less => (
                text.line_to_char_idx(anchor_line),
                text.line_to_char_idx(saturating_add(head_line, 1)),
            ),
            Ordering::Equal => match direction {
                Direction::Backward => (
                    text.line_to_char_idx(saturating_add(anchor_line, 1)),
                    text.line_to_char_idx(head_line),
                ),
                Direction::Forward => (
                    text.line_to_char_idx(head_line),
                    text.line_to_char_idx(saturating_add(anchor_line, 1)),
                ),
            },

            Ordering::Greater => (
                text.line_to_char_idx(saturating_add(anchor_line, 1)),
                text.line_to_char_idx(head_line),
            ),
        };
        Range::new(anchor, head)
    })
}

/// Expand each range to the document lines it touches.
pub fn extend_to_line_bounds(text: RopeSlice, selection: &Selection) -> Selection {
    selection.clone().transform(|range| {
        let (start_line, end_line) = range.line_range(text);
        let start = text.line_to_char_idx(start_line);
        let end = text.line_to_char_idx((end_line + 1).min(text.len_lines(stdx::rope::LINE_TYPE)));

        Range::new(start, end).with_direction(range.direction())
    })
}

/// Keep only fully selected document lines; leave single-line ranges unchanged.
pub fn shrink_to_line_bounds(text: RopeSlice, selection: &Selection) -> Selection {
    selection.clone().transform(|range| {
        let (start_line, end_line) = range.line_range(text);

        // Do nothing if the selection is within one line to prevent
        // conditional logic for the behavior of this command
        if start_line == end_line {
            return range;
        }

        let mut start = text.line_to_char_idx(start_line);

        // line_to_char gives us the start position of the line, so
        // we need to get the start position of the next line. In
        // the editor, this will correspond to the cursor being on
        // the EOL whitespace character, which is what we want.
        let mut end =
            text.line_to_char_idx((end_line + 1).min(text.len_lines(stdx::rope::LINE_TYPE)));

        if start != range.from() {
            start =
                text.line_to_char_idx((start_line + 1).min(text.len_lines(stdx::rope::LINE_TYPE)));
        }

        if end != range.to() {
            end = text.line_to_char_idx(end_line);
        }

        Range::new(start, end).with_direction(range.direction())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rope;
    use smallvec::smallvec;

    #[test]
    fn trimming_preserves_direction_and_keeps_a_primary_cursor() {
        let text = Rope::from("  λ  \t ");
        assert_eq!(
            trim(text.slice(..), &Selection::single(5, 0)),
            Selection::single(3, 2)
        );
        let selection = Selection::new(smallvec![Range::new(0, 2), Range::new(5, 7)], 1);
        assert_eq!(trim(text.slice(..), &selection), Selection::single(6, 6));
    }

    #[test]
    fn copying_uses_cell_columns_and_skips_short_lines() {
        let text = Rope::from("\t界e\u{301}\nx\n    界e\u{301}\n");
        let selection = Selection::single(4, 1);
        let copied = copy_on_line(text.slice(..), &selection, 4, Direction::Forward, 1);
        assert_eq!(copied.ranges(), &[Range::new(4, 1), Range::new(14, 11)]);
        assert_eq!(copied.primary_index(), 1);
        let at_start = copy_on_line(text.slice(..), &selection, 4, Direction::Backward, 2);
        assert_eq!(at_start, selection);
    }

    #[test]
    fn line_selection_counts_each_range_independently() {
        for ending in ["\n", "\r\n"] {
            let text = Rope::from(format!(
                "aa{ending}bb{ending}cc{ending}dd{ending}ee{ending}ff{ending}"
            ));
            let line = |n| text.line_to_char_idx(n);
            let selection =
                Selection::new(smallvec![Range::new(1, 2), Range::new(line(3), line(4))], 1);
            let selected = select_line(text.slice(..), &selection, Direction::Forward, 1);
            assert_eq!(
                selected.ranges(),
                &[Range::new(0, line(1)), Range::new(line(3), line(5))]
            );
            assert_eq!(selected.primary_index(), 1);
        }
    }

    #[test]
    fn line_bounds_preserve_backwards_selections_and_primary() {
        for ending in ["\n", "\r\n"] {
            let text = Rope::from(format!("aa{ending}bb{ending}cc{ending}dd"));
            let end = text.line_to_char_idx(2) + 1;
            let selection = Selection::single(end, 1);
            assert_eq!(
                extend_to_line_bounds(text.slice(..), &selection),
                Selection::single(text.line_to_char_idx(3), 0)
            );
            assert_eq!(
                shrink_to_line_bounds(text.slice(..), &selection),
                Selection::single(text.line_to_char_idx(2), text.line_to_char_idx(1))
            );
            let last = Selection::single(text.line_to_char_idx(3), text.len_chars());
            assert_eq!(
                extend_line(text.slice(..), &last, Direction::Forward, 3),
                last
            );
        }
    }
}
