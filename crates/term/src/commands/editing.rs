//! Text transformations over selections: case, alignment, line joining, and content order.

use std::char::{ToLowercase, ToUppercase};

use editor_core::{
    line_ending::line_end_char_index, movement as core_movement, Range, RopeSlice, Selection,
    SmallVec, Tendril, Transaction,
};

use super::{context::Context, continued_line_comment_token, exit_select_mode};

// align text in selection
#[allow(deprecated)]
pub(super) fn align_selections(cx: &mut Context) {
    use editor_core::visual_coords_at_pos;

    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id);

    let tab_width = doc.tab_width();

    let mut column_widths: Vec<usize> = Vec::new();
    let mut coordinates = Vec::with_capacity(selection.len());

    let mut previous_line = usize::MAX;
    let mut col_idx = 0;
    let mut running_offset = 0;

    for range in selection {
        let coords = visual_coords_at_pos(text, range.head, tab_width);
        let anchor_coords = visual_coords_at_pos(text, range.anchor, tab_width);

        if coords.row != anchor_coords.row {
            cx.editor
                .set_error(|| "align cannot work with multi line selections");
            return;
        }
        if coords.row != previous_line {
            col_idx = 0;
            running_offset = 0;
            previous_line = coords.row;
        }

        let width = coords.col - running_offset;

        match column_widths.get_mut(col_idx) {
            Some(n) => *n = (*n).max(width),
            None => column_widths.push(width),
        }

        coordinates.push(coords);

        running_offset += width;
        col_idx += 1;
    }

    let column_positions: Vec<_> = column_widths
        .into_iter()
        .scan(0, |sum, n| {
            *sum += n;
            Some(*sum)
        })
        .collect();

    previous_line = usize::MAX;

    let changes = coordinates
        .into_iter()
        .zip(selection)
        .map(|(coords, range)| {
            if coords.row != previous_line {
                col_idx = 0;
                running_offset = 0;
                previous_line = coords.row;
            }
            let current_inserts = column_positions[col_idx] - coords.col - running_offset;
            let insert_pos = range.from();

            col_idx += 1;
            running_offset += current_inserts;

            (
                insert_pos,
                insert_pos,
                Some(" ".repeat(current_inserts).into()),
            )
        });

    let transaction = Transaction::change(doc.text(), changes);
    doc.apply(&transaction, view.id);
    exit_select_mode(cx);
}

fn switch_case_impl<F>(cx: &mut Context, change_fn: F)
where
    F: Fn(RopeSlice) -> Tendril,
{
    let (view, doc) = current!(cx.editor);
    let selection = doc.selection(view.id);
    let transaction = Transaction::change_by_selection(doc.text(), selection, |range| {
        let text: Tendril = change_fn(range.slice(doc.text().slice(..)));

        (range.from(), range.to(), Some(text))
    });

    doc.apply(&transaction, view.id);
    exit_select_mode(cx);
}

enum CaseSwitcher {
    Upper(ToUppercase),
    Lower(ToLowercase),
    Keep(Option<char>),
}

impl Iterator for CaseSwitcher {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            CaseSwitcher::Upper(upper) => upper.next(),
            CaseSwitcher::Lower(lower) => lower.next(),
            CaseSwitcher::Keep(ch) => ch.take(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            CaseSwitcher::Upper(upper) => upper.size_hint(),
            CaseSwitcher::Lower(lower) => lower.size_hint(),
            CaseSwitcher::Keep(ch) => {
                let n = if ch.is_some() { 1 } else { 0 };
                (n, Some(n))
            }
        }
    }
}

impl ExactSizeIterator for CaseSwitcher {}

pub(super) fn switch_case(cx: &mut Context) {
    switch_case_impl(cx, |string| {
        string
            .chars()
            .flat_map(|ch| {
                if ch.is_lowercase() {
                    CaseSwitcher::Upper(ch.to_uppercase())
                } else if ch.is_uppercase() {
                    CaseSwitcher::Lower(ch.to_lowercase())
                } else {
                    CaseSwitcher::Keep(Some(ch))
                }
            })
            .collect()
    });
}

pub(super) fn switch_to_uppercase(cx: &mut Context) {
    switch_case_impl(cx, |string| {
        string.chunks().map(|chunk| chunk.to_uppercase()).collect()
    });
}

pub(super) fn switch_to_lowercase(cx: &mut Context) {
    switch_case_impl(cx, |string| {
        string.chunks().map(|chunk| chunk.to_lowercase()).collect()
    });
}

fn join_selections_impl(cx: &mut Context, select_space: bool) {
    use core_movement::skip_while;
    let loader = cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    let text = doc.text();
    let slice = text.slice(..);

    let mut changes = Vec::new();

    for selection in doc.selection(view.id) {
        let (start, mut end) = selection.line_range(slice);
        if start == end {
            end = (end + 1).min(text.len_lines() - 1);
        }
        let lines = start..end;

        changes.reserve(lines.len());

        let first_line_idx = slice.line_to_char(start);
        let first_line_idx = skip_while(slice, first_line_idx, |ch| matches!(ch, ' ' | '\t'))
            .unwrap_or(first_line_idx);
        let mut current_comment_token = continued_line_comment_token(
            doc,
            &loader,
            slice,
            start,
            slice.char_to_byte(first_line_idx),
        );

        for line in lines {
            let start = line_end_char_index(&slice, line);
            let mut end = text.line_to_char(line + 1);
            end = skip_while(slice, end, |ch| matches!(ch, ' ' | '\t')).unwrap_or(end);
            if let Some(token) =
                continued_line_comment_token(doc, &loader, slice, line + 1, slice.char_to_byte(end))
            {
                if Some(token) == current_comment_token {
                    end += token.chars().count();
                    end = skip_while(slice, end, |ch| matches!(ch, ' ' | '\t')).unwrap_or(end);
                } else {
                    // update current token, but don't delete this one.
                    current_comment_token = Some(token);
                }
            }

            let separator = if end == line_end_char_index(&slice, line + 1) {
                // the joining line contains only space-characters => don't include a whitespace when joining
                None
            } else {
                Some(Tendril::from(" "))
            };
            changes.push((start, end, separator));
        }
    }

    // nothing to do, bail out early to avoid crashes later
    if changes.is_empty() {
        return;
    }

    changes.sort_unstable_by_key(|(from, _to, _text)| *from);
    changes.dedup();

    // select inserted spaces
    let transaction = if select_space {
        let mut offset: usize = 0;
        let ranges: SmallVec<_> = changes
            .iter()
            .filter_map(|change| {
                if change.2.is_some() {
                    let range = Range::point(change.0 - offset);
                    offset += change.1 - change.0 - 1; // -1 adjusts for the replacement of the range by a space
                    Some(range)
                } else {
                    offset += change.1 - change.0;
                    None
                }
            })
            .collect();
        let t = Transaction::change(text, changes.into_iter());
        if ranges.is_empty() {
            t
        } else {
            let selection = Selection::new(ranges, 0);
            t.with_selection(selection)
        }
    } else {
        Transaction::change(text, changes.into_iter())
    };

    doc.apply(&transaction, view.id);
}

pub(super) fn join_selections(cx: &mut Context) {
    join_selections_impl(cx, false)
}

pub(super) fn join_selections_space(cx: &mut Context) {
    join_selections_impl(cx, true)
}

#[derive(Debug)]
enum ReorderStrategy {
    RotateForward,
    RotateBackward,
    Reverse,
}

fn reorder_selection_contents(cx: &mut Context, strategy: ReorderStrategy) {
    let count = cx.count;
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id);

    let mut ranges: Vec<_> = selection
        .slices(text)
        .map(|fragment| fragment.chunks().collect())
        .collect();

    let rotate_by = count.map_or(1, |count| count.get().min(ranges.len()));

    let primary_index = match strategy {
        ReorderStrategy::RotateForward => {
            ranges.rotate_right(rotate_by);
            // Like `usize::wrapping_add`, but provide a custom range from `0` to `ranges.len()`
            (selection.primary_index() + ranges.len() + rotate_by) % ranges.len()
        }
        ReorderStrategy::RotateBackward => {
            ranges.rotate_left(rotate_by);
            // Like `usize::wrapping_sub`, but provide a custom range from `0` to `ranges.len()`
            (selection.primary_index() + ranges.len() - rotate_by) % ranges.len()
        }
        ReorderStrategy::Reverse => {
            if rotate_by.is_multiple_of(2) {
                // nothing changed, if we reverse something an even
                // amount of times, the output will be the same
                return;
            }
            ranges.reverse();
            // -1 to turn 1-based len into 0-based index
            (ranges.len() - 1) - selection.primary_index()
        }
    };

    let transaction = Transaction::change(
        doc.text(),
        selection
            .ranges()
            .iter()
            .zip(ranges)
            .map(|(range, fragment)| (range.from(), range.to(), Some(fragment))),
    );

    doc.set_selection(
        view.id,
        Selection::new(selection.ranges().into(), primary_index),
    );
    doc.apply(&transaction, view.id);
}

pub(super) fn rotate_selection_contents_forward(cx: &mut Context) {
    reorder_selection_contents(cx, ReorderStrategy::RotateForward)
}
pub(super) fn rotate_selection_contents_backward(cx: &mut Context) {
    reorder_selection_contents(cx, ReorderStrategy::RotateBackward)
}
pub(super) fn reverse_selection_contents(cx: &mut Context) {
    reorder_selection_contents(cx, ReorderStrategy::Reverse)
}
