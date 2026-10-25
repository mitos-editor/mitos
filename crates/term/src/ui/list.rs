//! Cursor movement and viewport alignment shared by menus and pickers.

use editor_core::movement::Direction;

pub(super) fn move_cursor(
    cursor: Option<u32>,
    len: u32,
    amount: u32,
    direction: Direction,
) -> Option<u32> {
    if len == 0 {
        return cursor;
    }
    Some(match (cursor, direction) {
        (None, Direction::Forward) => 0,
        (None, Direction::Backward) => len.saturating_sub(amount) % len,
        (Some(cursor), Direction::Forward) => cursor.saturating_add(amount) % len,
        (Some(cursor), Direction::Backward) => {
            cursor.saturating_add(len).saturating_sub(amount) % len
        }
    })
}

pub(super) fn scroll_offset(cursor: usize, offset: usize, rows: usize) -> usize {
    offset
        .min(cursor)
        .max(cursor.saturating_sub(rows.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Direction::{Backward as B, Forward as F};

    #[test]
    fn movement_preserves_optional_selection_and_wraps_existing_cursors() {
        for (cursor, len, amount, direction, expected) in [
            (None, 0, 1, F, None),
            (None, 3, 2, F, Some(0)),
            (None, 3, 2, B, Some(1)),
            (Some(2), 3, 1, F, Some(0)),
            (Some(0), 3, 1, B, Some(2)),
            (Some(1), 3, 8, B, Some(0)),
        ] {
            assert_eq!(move_cursor(cursor, len, amount, direction), expected);
        }
    }

    #[test]
    fn scrolling_keeps_the_cursor_visible_without_moving_an_aligned_viewport() {
        for (cursor, offset, rows, expected) in
            [(5, 0, 3, 3), (1, 3, 3, 1), (4, 3, 3, 3), (2, 0, 0, 2)]
        {
            assert_eq!(scroll_offset(cursor, offset, rows), expected);
        }
    }
}
