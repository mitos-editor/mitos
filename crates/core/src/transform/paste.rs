//! Paste transactions over explicit text and selections.

use crate::{
    line_ending::{get_line_ending_of_str, normalize_line_endings},
    LineEnding, Range, Rope, Selection, SmallVec, Tendril, Transaction,
};
use stdx::rope::RopeExt as _;

/// Where pasted text is inserted relative to each selected range.
#[derive(Debug, Clone, Copy)]
pub enum PastePosition {
    Before,
    After,
    Cursor,
}

fn replacement_values<'a, T: AsRef<str>>(
    values: &'a [T],
    line_ending: LineEnding,
    count: usize,
) -> Option<impl Iterator<Item = Tendril> + 'a> {
    if count == 0 {
        return None;
    }
    let prepare = move |value: &str| {
        let value = normalize_line_endings(value, line_ending);
        let mut out = Tendril::from(value.as_ref());
        for _ in 1..count {
            out.push_str(&value);
        }
        out
    };
    let last = prepare(values.last()?.as_ref());
    Some(
        values
            .iter()
            .map(move |value| prepare(value.as_ref()))
            .chain(std::iter::repeat(last)),
    )
}

/// Build a paste transaction and the ranges of its inserted text.
///
/// Values are normalized to `line_ending`, repeated `count` times, and assigned
/// in selection order. If there are fewer values, reuse the last one. A value
/// ending in a newline makes before/after insertion linewise. Cursor insertion
/// remains characterwise. The returned selection preserves direction and primary.
/// An empty value list or a zero count returns `None`.
pub fn paste<T: AsRef<str>>(
    doc: &Rope,
    selection: &Selection,
    values: &[T],
    position: PastePosition,
    count: usize,
    line_ending: LineEnding,
) -> Option<(Transaction, Selection)> {
    let mut replacements = replacement_values(values, line_ending, count)?;
    let linewise = values
        .iter()
        .any(|value| get_line_ending_of_str(value.as_ref()).is_some());
    let mut offset = 0;
    let mut ranges = SmallVec::with_capacity(selection.len());
    let transaction = Transaction::change_by_selection(doc, selection, |range| {
        let pos = insert_position(doc, range, position, linewise);
        let value = replacements.next().unwrap(); // The last value repeats indefinitely.
        let len = value.chars().count();
        let start = pos + offset;
        ranges.push(Range::new(start, start + len).with_direction(range.direction()));
        offset += len;
        (pos, pos, Some(value))
    });
    Some((
        transaction,
        Selection::new(ranges, selection.primary_index()),
    ))
}

fn insert_position(doc: &Rope, range: &Range, position: PastePosition, linewise: bool) -> usize {
    match (position, linewise) {
        (PastePosition::Before, true) => doc.line_to_char_idx(doc.char_to_line_idx(range.from())),
        (PastePosition::After, true) => {
            let line = range.line_range(doc.slice(..)).1;
            doc.line_to_char_idx((line + 1).min(doc.len_lines(stdx::rope::LINE_TYPE)))
        }
        (PastePosition::Before, false) => range.from(),
        (PastePosition::After, false) => range.to(),
        (PastePosition::Cursor, _) => range.cursor(doc.slice(..)),
    }
}

/// Paste every value at each selected range, selecting each pasted value separately.
///
/// Unlike [`paste`], which gives each range one value, every range receives all
/// values in order. Values are normalized and repeated `count` times like [`paste`].
/// Characterwise values are separated by `line_ending`, which stays unselected, so
/// the pasted ranges never touch and are not merged. When any value ends in a
/// newline the paste is linewise: each value is completed to a whole line.
/// Empty values are skipped. The primary selection becomes the first value pasted
/// at the original primary range.
pub fn paste_all<T: AsRef<str>>(
    doc: &Rope,
    selection: &Selection,
    values: &[T],
    position: PastePosition,
    count: usize,
    line_ending: LineEnding,
) -> Option<(Transaction, Selection)> {
    let values: Vec<_> = values
        .iter()
        .map(AsRef::as_ref)
        .filter(|value| !value.is_empty())
        .collect();
    let linewise = values
        .iter()
        .any(|value| get_line_ending_of_str(value).is_some());
    let values: Vec<Tendril> = replacement_values(&values, line_ending, count)?
        .take(values.len())
        .map(|mut value| {
            if linewise && get_line_ending_of_str(&value).is_none() {
                value.push_str(line_ending.as_str());
            }
            value
        })
        .collect();
    let separator = if linewise { "" } else { line_ending.as_str() };
    let mut block = Tendril::new();
    let mut spans = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            block.push_str(separator);
        }
        let start = block.chars().count();
        block.push_str(value);
        spans.push((start, start + value.chars().count()));
    }
    let block_len = block.chars().count();

    let mut offset = 0;
    let mut ranges = SmallVec::with_capacity(selection.len() * spans.len());
    let transaction = Transaction::change_by_selection(doc, selection, |range| {
        let pos = insert_position(doc, range, position, linewise);
        let start = pos + offset;
        ranges.extend(spans.iter().map(|&(from, to)| {
            Range::new(start + from, start + to).with_direction(range.direction())
        }));
        offset += block_len;
        (pos, pos, Some(block.clone()))
    });
    Some((
        transaction,
        Selection::new(ranges, selection.primary_index() * spans.len()),
    ))
}

/// Replace nonempty selections with normalized, repeated values in selection order.
/// Empty ranges do not consume a value. Remaining ranges reuse the last value.
/// The transaction maps existing selections through the edits as usual.
pub fn replace_selection_contents<T: AsRef<str>>(
    doc: &Rope,
    selection: &Selection,
    values: &[T],
    count: usize,
    line_ending: LineEnding,
) -> Option<Transaction> {
    let mut replacements = replacement_values(values, line_ending, count)?;
    Some(Transaction::change_by_selection(doc, selection, |range| {
        let value = (!range.is_empty()).then(|| replacements.next().unwrap());
        (range.from(), range.to(), value)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smallvec;

    #[test]
    fn paste_preserves_character_offsets_direction_and_primary() {
        let mut text = Rope::from("a b c");
        let selection = Selection::new(
            smallvec![Range::new(0, 1), Range::new(3, 2), Range::new(4, 5)],
            1,
        );
        let (transaction, inserted) = paste(
            &text,
            &selection,
            &["界", "e\u{301}"],
            PastePosition::After,
            2,
            LineEnding::LF,
        )
        .unwrap();
        assert!(transaction.apply(&mut text));
        assert_eq!(text, "a界界 be\u{301}e\u{301} ce\u{301}e\u{301}");
        assert_eq!(
            inserted.ranges(),
            &[Range::new(1, 3), Range::new(9, 5), Range::new(11, 15)]
        );
        assert_eq!(inserted.primary_index(), 1);
    }

    #[test]
    fn linewise_paste_normalizes_line_endings_and_reaches_eof() {
        for ending in [LineEnding::LF, LineEnding::Crlf] {
            let newline = ending.as_str();
            let original = Rope::from(format!("a{newline}b"));
            for (position, expected) in [
                (PastePosition::Before, format!("a{newline}界{newline}b")),
                (PastePosition::After, format!("a{newline}b界{newline}")),
            ] {
                let mut text = original.clone();
                let selection = Selection::single(text.len_chars() - 1, text.len_chars());
                let (transaction, inserted) =
                    paste(&text, &selection, &["界\r\n"], position, 1, ending).unwrap();
                assert!(transaction.apply(&mut text));
                assert_eq!(text, expected.as_str());
                assert_eq!(
                    inserted.primary().fragment(text.slice(..)),
                    format!("界{newline}")
                );
            }
        }
    }

    #[test]
    fn paste_all_selects_each_value_separately_at_every_range() {
        let mut text = Rope::from("ab\n");
        let selection = Selection::new(smallvec![Range::new(0, 1), Range::new(2, 1)], 1);
        let (transaction, inserted) = paste_all(
            &text,
            &selection,
            &["界", "", "x\r\ny"],
            PastePosition::After,
            1,
            LineEnding::LF,
        )
        .unwrap();
        assert!(transaction.apply(&mut text));
        // The paste after `a` holds both values; the separator newline is not selected.
        assert_eq!(text, "a界\nx\nyb界\nx\ny\n");
        let fragments: Vec<_> = inserted.fragments(text.slice(..)).collect();
        assert_eq!(fragments, ["界", "x\ny", "界", "x\ny"]);
        assert_eq!(inserted.ranges()[2], Range::new(8, 7));
        assert_eq!(inserted.primary_index(), 2);
    }

    #[test]
    fn linewise_paste_all_completes_every_value_to_a_line_and_repeats_by_count() {
        let mut text = Rope::from("a\r\nb\r\n");
        let selection = Selection::single(0, 1);
        let (transaction, inserted) = paste_all(
            &text,
            &selection,
            &["one\n", "two"],
            PastePosition::After,
            2,
            LineEnding::Crlf,
        )
        .unwrap();
        assert!(transaction.apply(&mut text));
        assert_eq!(text, "a\r\none\r\none\r\ntwotwo\r\nb\r\n");
        let fragments: Vec<_> = inserted.fragments(text.slice(..)).collect();
        assert_eq!(fragments, ["one\r\none\r\n", "twotwo\r\n"]);
    }

    #[test]
    fn paste_all_needs_a_nonempty_value() {
        let text = Rope::from("a");
        let selection = Selection::single(0, 1);
        for values in [&[] as &[&str], &["", ""]] {
            assert!(paste_all(
                &text,
                &selection,
                values,
                PastePosition::After,
                1,
                LineEnding::LF
            )
            .is_none());
        }
    }

    #[test]
    fn replacement_skips_cursors_and_reuses_the_last_value() {
        let mut text = Rope::from("a b c");
        let selection = Selection::new(
            smallvec![Range::point(0), Range::new(2, 3), Range::new(4, 5)],
            0,
        );
        let transaction =
            replace_selection_contents(&text, &selection, &["界\n"], 2, LineEnding::Crlf).unwrap();
        assert!(transaction.apply(&mut text));
        assert_eq!(text, "a 界\r\n界\r\n 界\r\n界\r\n");
    }

    #[test]
    fn absent_values_and_zero_counts_do_not_produce_edits() {
        let text = Rope::from("a");
        let selection = Selection::single(0, 1);
        assert!(paste(
            &text,
            &selection,
            &[] as &[&str],
            PastePosition::Before,
            1,
            LineEnding::LF
        )
        .is_none());
        assert!(replace_selection_contents(&text, &selection, &["x"], 0, LineEnding::LF).is_none());
    }
}
