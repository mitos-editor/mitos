//! Transactions that transform each selected fragment or reorder selected contents.

use crate::{
    increment, line_ending::normalize_line_endings, wrap::reflow_hard_wrap, LineEnding, Range,
    Rope, Selection, Transaction,
};
use stdx::rope::RopeSliceExt;

/// Replace each selected grapheme with the same literal text. Empty ranges are unchanged.
pub fn replace_graphemes(doc: &Rope, selection: &Selection, replacement: &str) -> Transaction {
    Transaction::change_by_selection(doc, selection, |range| {
        let text = range.slice(doc.slice(..));
        if range.is_empty() {
            return (range.from(), range.from(), None);
        }
        let replacement = text.graphemes().map(|_| replacement).collect();
        (range.from(), range.to(), Some(replacement))
    })
}

/// Increment selected integers or dates, preserving direction and the primary range.
/// `step` advances the amount after each range, including ranges that cannot be incremented.
/// Amounts saturate at the signed 64-bit limits. Returns `None` if nothing matched.
pub fn increment_selections(
    doc: &Rope,
    selection: &Selection,
    mut amount: i64,
    step: i64,
) -> Option<Transaction> {
    let text = doc.slice(..);
    let transaction = Transaction::change_by_and_with_selection(doc, selection, |range| {
        let selected = range.fragment(text);
        let incremented = [increment::integer, increment::date_time]
            .iter()
            .find_map(|increment| increment(&selected, amount));
        amount = amount.saturating_add(step);
        match incremented {
            Some(replacement) => {
                let end = Range::new(range.from(), range.from() + replacement.chars().count())
                    .with_direction(range.direction());
                (
                    (range.from(), range.to(), Some(replacement.into())),
                    Some(end),
                )
            }
            None => ((range.from(), range.from(), None), Some(*range)),
        }
    });
    (!transaction.changes().is_empty()).then_some(transaction)
}

/// Reflow selected text to the requested width and normalize its line endings.
pub fn reflow_selections(
    doc: &Rope,
    selection: &Selection,
    width: usize,
    line_ending: LineEnding,
) -> Transaction {
    Transaction::change_by_selection(doc, selection, |range| {
        let reflowed = reflow_hard_wrap(&range.fragment(doc.slice(..)), width);
        let replacement = normalize_line_endings(&reflowed, line_ending)
            .as_ref()
            .into();
        (range.from(), range.to(), Some(replacement))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test,
        transform::{sort_selection_contents, SortOptions},
    };

    fn assert_transform(
        input: &str,
        expected: &str,
        transform: impl FnOnce(&Rope, &Selection) -> Transaction,
    ) {
        let (text, selection) = test::print(input);
        let mut doc = Rope::from(text);
        let transaction = transform(&doc, &selection);
        let result = transaction
            .selection()
            .cloned()
            .unwrap_or_else(|| selection.map(transaction.changes()));
        assert!(transaction.apply(&mut doc));
        let (text, selection) = test::print(expected);
        assert_eq!(doc, text.as_str());
        assert_eq!(result, selection);
    }

    #[test]
    fn replacement_counts_graphemes_and_preserves_range_directions() {
        assert_transform(
            "#[|e\u{301}]# #(界|)#\r\n",
            "#[|🙂]# #(🙂|)#\r\n",
            |doc, selection| replace_graphemes(doc, selection, "🙂"),
        );
        assert_transform("#[|a]# #(b|)#", "#[|\r\n]# #(\r\n|)#", |doc, selection| {
            replace_graphemes(doc, selection, "\r\n")
        });
        let doc = Rope::from_str("");
        let transaction = replace_graphemes(&doc, &Selection::point(0), "x");
        assert!(transaction.changes().is_empty());
    }

    #[test]
    fn increments_map_length_changes_and_keep_nonmatching_unicode_ranges() {
        assert_transform(
            "#[|9]# #(界|)# #(99|)#",
            "#[|10]# #(界|)# #(102|)#",
            |doc, selection| increment_selections(doc, selection, 1, 1).unwrap(),
        );
        assert_transform(
            "#(2026-10-20|)# #[|9]#",
            "#(2026-10-21|)# #[|10]#",
            |doc, selection| increment_selections(doc, selection, 1, 0).unwrap(),
        );
        let doc = Rope::from_str("界");
        assert!(increment_selections(&doc, &Selection::single(0, 1), 1, 0).is_none());
    }

    #[test]
    fn stepped_increments_saturate_at_the_amount_limit() {
        assert_transform(
            "#[0|]# #(0|)#",
            "#[9223372036854775807|]# #(9223372036854775807|)#",
            |doc, selection| increment_selections(doc, selection, i64::MAX, 1).unwrap(),
        );
    }

    #[test]
    fn case_insensitive_sorting_is_stable_in_both_directions() {
        let input = "#[|é]# #(B|)# #(b|)# #(a|)#";
        for (reverse, expected) in [
            (false, "#[|a]# #(B|)# #(b|)# #(é|)#"),
            (true, "#[|é]# #(B|)# #(b|)# #(a|)#"),
        ] {
            assert_transform(input, expected, |doc, selection| {
                sort_selection_contents(
                    doc,
                    selection,
                    &SortOptions {
                        case_insensitive: true,
                        reverse,
                        ..SortOptions::default()
                    },
                )
                .unwrap()
            });
        }
        let doc = Rope::from_str("one");
        assert!(
            sort_selection_contents(&doc, &Selection::single(0, 3), &SortOptions::default())
                .is_none()
        );
    }

    #[test]
    fn sorting_maps_ranges_when_fragment_lengths_differ() {
        assert_transform(
            "#(b|)# #[|aa]# #(ZZZ|)#",
            "#(ZZZ|)# #[|aa]# #(b|)#",
            |doc, selection| {
                sort_selection_contents(doc, selection, &SortOptions::default()).unwrap()
            },
        );
    }

    #[test]
    fn reflow_uses_the_requested_line_endings() {
        for (ending, expected) in [
            (LineEnding::LF, "#[|one two\nthree]#"),
            (LineEnding::Crlf, "#[|one two\r\nthree]#"),
        ] {
            assert_transform("#[|one two three]#", expected, |doc, selection| {
                reflow_selections(doc, selection, 7, ending)
            });
        }
    }
}
