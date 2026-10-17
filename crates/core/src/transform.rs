//! Text transformations expressed as transactions over explicit selections.

use crate::{indent::IndentStyle, Rope, RopeSlice, Selection, Tendril, Transaction};
use std::char::{ToLowercase, ToUppercase};
use stdx::rope::RopeSliceExt;

/// Align single-line selections to shared columns; return `None` for multiline ranges.
#[allow(deprecated)]
pub fn align_selections(
    doc: &Rope,
    selection: &Selection,
    tab_width: usize,
) -> Option<Transaction> {
    use crate::visual_coords_at_pos;

    let text = doc.slice(..);

    let mut column_widths: Vec<usize> = Vec::new();
    let mut coordinates = Vec::with_capacity(selection.len());

    let mut previous_line = usize::MAX;
    let mut col_idx = 0;
    let mut running_offset = 0;

    for range in selection {
        let coords = visual_coords_at_pos(text, range.head, tab_width);
        let anchor_coords = visual_coords_at_pos(text, range.anchor, tab_width);

        if coords.row != anchor_coords.row {
            return None;
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

    Some(Transaction::change(doc, changes))
}

fn map_case<F>(doc: &Rope, selection: &Selection, change_fn: F) -> Transaction
where
    F: Fn(RopeSlice) -> Tendril,
{
    Transaction::change_by_selection(doc, selection, |range| {
        let text: Tendril = change_fn(range.slice(doc.slice(..)));

        (range.from(), range.to(), Some(text))
    })
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

/// Swap upper and lower case within each selected range.
pub fn switch_case(doc: &Rope, selection: &Selection) -> Transaction {
    map_case(doc, selection, |string| {
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
    })
}

/// Convert each selected range to uppercase.
pub fn switch_to_uppercase(doc: &Rope, selection: &Selection) -> Transaction {
    map_case(doc, selection, |string| {
        string.chunks().map(|chunk| chunk.to_uppercase()).collect()
    })
}

/// Convert each selected range to lowercase.
pub fn switch_to_lowercase(doc: &Rope, selection: &Selection) -> Transaction {
    map_case(doc, selection, |string| {
        string.chunks().map(|chunk| chunk.to_lowercase()).collect()
    })
}

/// How to reorder the contents of the selected ranges.
#[derive(Debug)]
pub enum ReorderStrategy {
    RotateForward,
    RotateBackward,
    Reverse,
}

/// Return the edit and its primary selection in pre-edit coordinates.
/// Set that selection before applying the transaction.
pub fn reorder_selection_contents(
    doc: &Rope,
    selection: &Selection,
    strategy: ReorderStrategy,
    count: usize,
) -> Option<(Transaction, Selection)> {
    let text = doc.slice(..);

    let mut ranges: Vec<_> = selection
        .slices(text)
        .map(|fragment| fragment.chunks().collect())
        .collect();

    let rotate_by = count.min(ranges.len());

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
                return None;
            }
            ranges.reverse();
            // -1 to turn 1-based len into 0-based index
            (ranges.len() - 1) - selection.primary_index()
        }
    };

    let transaction = Transaction::change(
        doc,
        selection
            .ranges()
            .iter()
            .zip(ranges)
            .map(|(range, fragment)| (range.from(), range.to(), Some(fragment))),
    );

    Some((
        transaction,
        Selection::new(selection.ranges().into(), primary_index),
    ))
}

fn selected_lines(doc: &Rope, selection: &Selection) -> Vec<usize> {
    let mut lines = Vec::new();

    // Get all line numbers
    for range in selection {
        let (start, end) = range.line_range(doc.slice(..));

        for line in start..=end {
            lines.push(line)
        }
    }
    lines.sort_unstable(); // sorting by usize so _unstable is preferred
    lines.dedup();
    lines
}

/// Indent nonblank selected lines, adjusting partial space indents.
pub fn indent(doc: &Rope, selection: &Selection, style: IndentStyle, count: usize) -> Transaction {
    let lines = selected_lines(doc, selection);

    // Indent by one level
    let indent = Tendril::from(style.as_str().repeat(count));

    Transaction::change(
        doc,
        lines.into_iter().filter_map(|line| {
            let is_blank = doc.line(line).chunks().all(|s| s.trim().is_empty());
            if is_blank {
                return None;
            }
            let pos = doc.line_to_char(line);

            let indent = if let IndentStyle::Spaces(indent_width) = style {
                let line = doc.line(line);
                let offset = line.first_non_whitespace_char().unwrap_or(0) % indent_width as usize;
                indent.clone().split_off(offset)
            } else {
                indent.clone()
            };

            Some((pos, pos, Some(indent)))
        }),
    )
}

/// Remove leading whitespace up to the requested visual indent width.
pub fn unindent(
    doc: &Rope,
    selection: &Selection,
    tab_width: usize,
    indent_width: usize,
) -> Transaction {
    let lines = selected_lines(doc, selection);
    let mut changes = Vec::with_capacity(lines.len());

    for line_idx in lines {
        let line = doc.line(line_idx);
        let mut width = 0;
        let mut pos = 0;

        for ch in line.chars() {
            match ch {
                ' ' => width += 1,
                '\t' => width = (width / tab_width + 1) * tab_width,
                _ => break,
            }

            pos += 1;

            if width >= indent_width {
                break;
            }
        }

        // now delete from start to first non-blank
        if pos > 0 {
            let start = doc.line_to_char(line_idx);
            changes.push((start, start + pos, None))
        }
    }

    Transaction::change(doc, changes.into_iter())
}
