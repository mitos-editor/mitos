//! Insert-mode deletion with indentation, pairing, and append-selection semantics.

use crate::{Document, ViewId};
use editor_core::{
    auto_pairs,
    auto_pairs::AutoPairs,
    graphemes,
    line_ending::line_end_char_index,
    movement::{self as core_movement, Direction},
    Deletion, Range, RopeSlice, Selection, SmallVec, Transaction,
};
use language::syntax;
use std::borrow::Cow;
use stdx::rope::RopeSliceExt;

/// Exclude the cursor in range.
fn exclude_cursor(text: RopeSlice, range: Range, cursor: Range) -> Range {
    if range.to() == cursor.to() && text.len_chars() != cursor.to() {
        Range::new(
            range.from(),
            graphemes::prev_grapheme_boundary(text, cursor.to()),
        )
    } else {
        range
    }
}

fn dedent(doc: &Document, range: &Range) -> Option<Deletion> {
    let text = doc.text().slice(..);
    let pos = range.cursor(text);
    let line_start_pos = text.line_to_char(range.cursor_line(text));

    // consider to delete by indent level if all characters before `pos` are indent units.
    let fragment = Cow::from(text.slice(line_start_pos..pos));

    if fragment.is_empty() || !fragment.chars().all(|ch| ch == ' ' || ch == '\t') {
        return None;
    }

    if text.get_char(pos.saturating_sub(1)) == Some('\t') {
        // fast path, delete one char
        return Some((graphemes::nth_prev_grapheme_boundary(text, pos, 1), pos));
    }

    let tab_width = doc.tab_width();
    let indent_width = doc.indent_width();

    let width: usize = fragment
        .chars()
        .map(|ch| if ch == '\t' { tab_width } else { 1 })
        .sum();

    // round down to nearest unit
    let mut drop = width % indent_width;

    // if it's already at a unit, consume a whole unit
    if drop == 0 {
        drop = indent_width
    };

    let mut chars = fragment.chars().rev();
    let mut start = pos;

    for _ in 0..drop {
        // delete up to `drop` spaces
        match chars.next() {
            Some(' ') => start -= 1,
            _ => break,
        }
    }

    Some((start, pos)) // delete!
}

/// Delete backward by indentation, auto pair, or grapheme boundaries.
pub fn delete_char_backward(
    doc: &mut Document,
    view_id: ViewId,
    count: usize,
    loader: &syntax::Loader,
    global_pairs: Option<&AutoPairs>,
) {
    let text = doc.text().slice(..);

    let auto_pairs = doc.auto_pairs(global_pairs, loader, doc.selection(view_id));

    let transaction =
        Transaction::delete_by_and_with_selection(doc.text(), doc.selection(view_id), |range| {
            let pos = range.cursor(text);

            if pos == 0 {
                return ((pos, pos), None);
            }

            dedent(doc, range)
                .map(|dedent| (dedent, None))
                .or_else(|| {
                    // Pair configuration follows the primary selection's language.
                    auto_pairs::hook_delete(doc.text(), range, auto_pairs?)
                        .map(|(delete, new_range)| (delete, Some(new_range)))
                })
                .unwrap_or_else(|| {
                    (
                        (graphemes::nth_prev_grapheme_boundary(text, pos, count), pos),
                        None,
                    )
                })
        });

    super::super::apply(doc, &transaction, view_id);
}

/// Delete forward while retaining append-mode selections.
pub fn delete_char_forward(doc: &mut Document, view_id: ViewId, count: usize) {
    delete_by_selection_insert_mode(
        doc,
        view_id,
        |text, range| {
            let pos = range.cursor(text);
            (pos, graphemes::nth_next_grapheme_boundary(text, pos, count))
        },
        Direction::Forward,
    )
}

/// Delete backward to word starts.
pub fn delete_word_backward(doc: &mut Document, view_id: ViewId, count: usize) {
    delete_by_selection_insert_mode(
        doc,
        view_id,
        |text, range| {
            let anchor = core_movement::move_prev_word_start(text, *range, count).from();
            let next = Range::new(anchor, range.cursor(text));
            let range = exclude_cursor(text, next, *range);
            (range.from(), range.to())
        },
        Direction::Backward,
    );
}

/// Delete forward to word ends while retaining append-mode selections.
pub fn delete_word_forward(doc: &mut Document, view_id: ViewId, count: usize) {
    delete_by_selection_insert_mode(
        doc,
        view_id,
        |text, range| {
            let head = core_movement::move_next_word_end(text, *range, count).to();
            (range.cursor(text), head)
        },
        Direction::Forward,
    );
}

fn delete_by_selection_insert_mode(
    doc: &mut Document,
    view_id: ViewId,
    mut f: impl FnMut(RopeSlice, &Range) -> Deletion,
    direction: Direction,
) {
    let text = doc.text().slice(..);
    let mut selection = SmallVec::new();
    let mut insert_newline = false;
    let text_len = text.len_chars();
    let mut transaction =
        Transaction::delete_by_selection(doc.text(), doc.selection(view_id), |range| {
            let (start, end) = f(text, range);
            if direction == Direction::Forward {
                let mut range = *range;
                if range.head > range.anchor {
                    insert_newline |= end == text_len;
                    // move the cursor to the right so that the selection
                    // doesn't shrink when deleting forward (so the text appears to
                    // move to  left)
                    // += 1 is enough here as the range is normalized to grapheme boundaries
                    // later anyway
                    range.head += 1;
                }
                selection.push(range);
            }
            (start, end)
        });

    // in case we delete the last character and the cursor would be moved to the EOF char
    // insert a newline, just like when entering append mode
    if insert_newline {
        transaction = transaction.insert_at_eof(doc.line_ending.as_str().into());
    }

    if direction == Direction::Forward {
        doc.set_selection(
            view_id,
            Selection::new(selection, doc.selection(view_id).primary_index()),
        );
    }
    super::super::apply(doc, &transaction, view_id);
}

/// Delete to the first nonblank character or the previous line ending.
pub fn kill_to_line_start(doc: &mut Document, view_id: ViewId) {
    delete_by_selection_insert_mode(
        doc,
        view_id,
        move |text, range| {
            let line = range.cursor_line(text);
            let first_char = text.line_to_char(line);
            let anchor = range.cursor(text);
            let head = if anchor == first_char && line != 0 {
                // select until previous line
                line_end_char_index(&text, line - 1)
            } else if let Some(pos) = text.line(line).first_non_whitespace_char() {
                if first_char + pos < anchor {
                    // select until first non-blank in line if cursor is after it
                    first_char + pos
                } else {
                    // select until start of line
                    first_char
                }
            } else {
                // select until start of line
                first_char
            };
            (head, anchor)
        },
        Direction::Backward,
    );
}

/// Delete through the line end, joining lines when already at the line ending.
pub fn kill_to_line_end(doc: &mut Document, view_id: ViewId) {
    delete_by_selection_insert_mode(
        doc,
        view_id,
        |text, range| {
            let line = range.cursor_line(text);
            let line_end_pos = line_end_char_index(&text, line);
            let pos = range.cursor(text);

            // if the cursor is on the newline char delete that
            if pos == line_end_pos {
                (pos, text.line_to_char(line + 1))
            } else {
                (pos, line_end_pos)
            }
        },
        Direction::Forward,
    );
}
