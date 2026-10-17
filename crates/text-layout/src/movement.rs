//! Cursor movement through formatted text and display annotations.

use crate::{
    char_idx_at_visual_block_offset, char_idx_at_visual_offset, doc_formatter::TextFormat,
    text_annotations::TextAnnotations, visual_offset_from_block,
};
use editor_core::{
    graphemes::prev_grapheme_boundary,
    movement::{Direction, Movement},
    Range, RopeSlice,
};

pub fn move_vertically_visual(
    slice: RopeSlice,
    range: Range,
    dir: Direction,
    count: usize,
    behaviour: Movement,
    text_fmt: &TextFormat,
    annotations: &mut TextAnnotations,
) -> Range {
    if !text_fmt.soft_wrap {
        return move_vertically(slice, range, dir, count, behaviour, text_fmt, annotations);
    }
    annotations.clear_line_annotations();
    let pos = range.cursor(slice);

    // Compute the current position's 2d coordinates.
    let (visual_pos, block_off) = visual_offset_from_block(slice, pos, pos, text_fmt, annotations);
    let new_col = range
        .old_visual_position
        .map_or(visual_pos.col as u32, |(_, col)| col);

    // Compute the new position.
    let mut row_off = match dir {
        Direction::Forward => count as isize,
        Direction::Backward => -(count as isize),
    };

    // Compute visual offset relative to block start to avoid trasversing the block twice
    row_off += visual_pos.row as isize;
    let (mut new_pos, virtual_rows) = char_idx_at_visual_offset(
        slice,
        block_off,
        row_off,
        new_col as usize,
        text_fmt,
        annotations,
    );
    if dir == Direction::Forward {
        new_pos += (virtual_rows != 0) as usize;
    }

    // Special-case to avoid moving to the end of the last non-empty line.
    if behaviour == Movement::Extend && slice.line(slice.char_to_line(new_pos)).len_chars() == 0 {
        return range;
    }

    let mut new_range = range.put_cursor(slice, new_pos, behaviour == Movement::Extend);
    new_range.old_visual_position = Some((0, new_col));
    new_range
}

pub fn move_vertically(
    slice: RopeSlice,
    range: Range,
    dir: Direction,
    count: usize,
    behaviour: Movement,
    text_fmt: &TextFormat,
    annotations: &mut TextAnnotations,
) -> Range {
    annotations.clear_line_annotations();
    let pos = range.cursor(slice);
    let line_idx = slice.char_to_line(pos);
    let line_start = slice.line_to_char(line_idx);

    // Compute the current position's 2d coordinates.
    let visual_pos = visual_offset_from_block(slice, line_start, pos, text_fmt, annotations).0;
    let (mut new_row, new_col) = range
        .old_visual_position
        .map_or((visual_pos.row as u32, visual_pos.col as u32), |pos| pos);
    new_row = new_row.max(visual_pos.row as u32);
    let line_idx = slice.char_to_line(pos);

    // Compute the new position.
    let mut new_line_idx = match dir {
        Direction::Forward => line_idx.saturating_add(count),
        Direction::Backward => line_idx.saturating_sub(count),
    };

    let line = if new_line_idx >= slice.len_lines() - 1 {
        // there is no line terminator for the last line
        // so the logic below is not necessary here
        new_line_idx = slice.len_lines() - 1;
        slice
    } else {
        // char_idx_at_visual_block_offset returns a one-past-the-end index
        // in case it reaches the end of the slice
        // to avoid moving to the nextline in that case the line terminator is removed from the line
        let new_line_end = prev_grapheme_boundary(slice, slice.line_to_char(new_line_idx + 1));
        slice.slice(..new_line_end)
    };

    let new_line_start = line.line_to_char(new_line_idx);

    let (new_pos, _) = char_idx_at_visual_block_offset(
        line,
        new_line_start,
        new_row as usize,
        new_col as usize,
        text_fmt,
        annotations,
    );

    // Special-case to avoid moving to the end of the last non-empty line.
    if behaviour == Movement::Extend && slice.line(new_line_idx).len_chars() == 0 {
        return range;
    }

    let mut new_range = range.put_cursor(slice, new_pos, behaviour == Movement::Extend);
    new_range.old_visual_position = Some((new_row, new_col));
    new_range
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{coords_at_pos, movement::move_horizontally, pos_at_coords, Rope};

    const MULTILINE_SAMPLE: &str = "\
        Multiline\n\
        text sample\n\
        which\n\
        is merely alphabetic\n\
        and whitespaced\n\
    ";

    const MULTIBYTE_CHARACTER_SAMPLE: &str = "\
        パーティーへ行かないか\n\
        The text above is Japanese\n\
    ";

    #[test]
    fn test_vertical_move() {
        let text = Rope::from("abcd\nefg\nwrs");
        let slice = text.slice(..);
        let pos = pos_at_coords(slice, (0, 4).into(), true);

        let range = Range::new(pos, pos);
        assert_eq!(
            coords_at_pos(
                slice,
                move_vertically_visual(
                    slice,
                    range,
                    Direction::Forward,
                    1,
                    Movement::Move,
                    &TextFormat::default(),
                    &mut TextAnnotations::default()
                )
                .head
            ),
            (1, 3).into()
        );
    }

    #[test]
    fn vertical_moves_in_single_column() {
        let text = Rope::from(MULTILINE_SAMPLE);
        let slice = text.slice(..);
        let position = pos_at_coords(slice, (0, 0).into(), true);
        let mut range = Range::point(position);
        let moves_and_expected_coordinates = [
            ((Direction::Forward, 1usize), (1, 0)),
            ((Direction::Forward, 2usize), (3, 0)),
            ((Direction::Forward, 1usize), (4, 0)),
            ((Direction::Backward, 999usize), (0, 0)),
            ((Direction::Forward, 4usize), (4, 0)),
            ((Direction::Forward, 0usize), (4, 0)),
            ((Direction::Backward, 0usize), (4, 0)),
            ((Direction::Forward, 5), (5, 0)),
            ((Direction::Forward, 999usize), (5, 0)),
        ];

        for ((direction, amount), coordinates) in moves_and_expected_coordinates {
            range = move_vertically_visual(
                slice,
                range,
                direction,
                amount,
                Movement::Move,
                &TextFormat::default(),
                &mut TextAnnotations::default(),
            );
            assert_eq!(coords_at_pos(slice, range.head), coordinates.into());
            assert_eq!(range.head, range.anchor);
        }
    }

    #[test]
    fn vertical_moves_jumping_column() {
        let text = Rope::from(MULTILINE_SAMPLE);
        let slice = text.slice(..);
        let position = pos_at_coords(slice, (0, 0).into(), true);
        let mut range = Range::point(position);

        enum Axis {
            H,
            V,
        }
        let moves_and_expected_coordinates = [
            // Places cursor at the end of line
            ((Axis::H, Direction::Forward, 8usize), (0, 8)),
            // First descent preserves column as the target line is wider
            ((Axis::V, Direction::Forward, 1usize), (1, 8)),
            // Second descent clamps column as the target line is shorter
            ((Axis::V, Direction::Forward, 1usize), (2, 5)),
            // Third descent restores the original column
            ((Axis::V, Direction::Forward, 1usize), (3, 8)),
            // Behaviour is preserved even through long jumps
            ((Axis::V, Direction::Backward, 999usize), (0, 8)),
            ((Axis::V, Direction::Forward, 4usize), (4, 8)),
            ((Axis::V, Direction::Forward, 999usize), (5, 0)),
        ];

        for ((axis, direction, amount), coordinates) in moves_and_expected_coordinates {
            range = match axis {
                Axis::H => move_horizontally(slice, range, direction, amount, Movement::Move),
                Axis::V => move_vertically_visual(
                    slice,
                    range,
                    direction,
                    amount,
                    Movement::Move,
                    &TextFormat::default(),
                    &mut TextAnnotations::default(),
                ),
            };
            assert_eq!(coords_at_pos(slice, range.head), coordinates.into());
            assert_eq!(range.head, range.anchor);
        }
    }

    #[test]
    fn multibyte_character_wide_column_jumps() {
        let text = Rope::from(MULTIBYTE_CHARACTER_SAMPLE);
        let slice = text.slice(..);
        let position = pos_at_coords(slice, (0, 0).into(), true);
        let mut range = Range::point(position);

        // FIXME: The behaviour captured in this test diverges from both Kakoune and Vim. These
        // will attempt to preserve the horizontal position of the cursor, rather than
        // placing it at the same character index.
        enum Axis {
            H,
            V,
        }
        let moves_and_expected_coordinates = [
            // Places cursor at the fourth kana.
            ((Axis::H, Direction::Forward, 4), (0, 4)),
            // Descent places cursor at the 8th character.
            ((Axis::V, Direction::Forward, 1usize), (1, 8)),
            // Moving back 2 characters.
            ((Axis::H, Direction::Backward, 2usize), (1, 6)),
            // Jumping back up 1 line.
            ((Axis::V, Direction::Backward, 1usize), (0, 3)),
        ];

        for ((axis, direction, amount), coordinates) in moves_and_expected_coordinates {
            range = match axis {
                Axis::H => move_horizontally(slice, range, direction, amount, Movement::Move),
                Axis::V => move_vertically_visual(
                    slice,
                    range,
                    direction,
                    amount,
                    Movement::Move,
                    &TextFormat::default(),
                    &mut TextAnnotations::default(),
                ),
            };
            assert_eq!(coords_at_pos(slice, range.head), coordinates.into());
            assert_eq!(range.head, range.anchor);
        }
    }
}
