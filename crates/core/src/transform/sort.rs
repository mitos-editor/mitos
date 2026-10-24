//! Stable sorting of selected fragments using independently computed comparison keys.

use std::{
    borrow::Cow,
    cmp::{Ordering, Reverse},
};

use regex::Regex;

use crate::{Range, Rope, Selection, Tendril, Transaction};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    #[default]
    Text,
    Numeric,
    Natural,
    Length,
}

#[derive(Debug, Default)]
pub struct SortOptions<'a> {
    pub mode: SortMode,
    pub case_insensitive: bool,
    pub reverse: bool,
    pub ignore_leading_blanks: bool,
    /// Compare the first full regex match; leave nonmatching selections in place.
    pub key: Option<&'a Regex>,
}

/// Stably sort selected contents, preserving text outside the selected ranges.
/// Keys are extracted and normalized once per fragment. Equal keys retain input order,
/// including in reverse mode. Length counts Unicode scalar values before case conversion.
/// Returns `None` when fewer than two ranges were supplied.
pub fn sort_selection_contents(
    doc: &Rope,
    selection: &Selection,
    options: &SortOptions<'_>,
) -> Option<Transaction> {
    if selection.len() < 2 {
        return None;
    }
    let mut fragments: Vec<Tendril> = selection
        .slices(doc.slice(..))
        .map(|fragment| fragment.chunks().collect())
        .collect();
    // Preserve the existing fast path for the default command and its original flags.
    if options.mode == SortMode::Text && options.key.is_none() && !options.ignore_leading_blanks {
        match (options.case_insensitive, options.reverse) {
            (true, true) => fragments.sort_by_cached_key(|text| Reverse(text.to_lowercase())),
            (true, false) => fragments.sort_by_cached_key(|text| text.to_lowercase()),
            (false, true) => fragments.sort_by(|a, b| b.cmp(a)),
            (false, false) => fragments.sort(),
        }
        return Some(Transaction::change(
            doc,
            selection
                .iter()
                .zip(fragments)
                .map(|(range, text)| (range.from(), range.to(), Some(text))),
        ));
    }
    let mut keys: Vec<_> = fragments
        .iter()
        .enumerate()
        .filter_map(|(index, fragment)| {
            let text = match options.key {
                Some(pattern) => pattern.find(fragment)?.as_str(),
                None => fragment.as_str(),
            };
            let text = if options.ignore_leading_blanks {
                text.trim_start_matches([' ', '\t'])
            } else {
                text
            };
            Some((index, SortKey::new(text, options)))
        })
        .collect();
    let destinations: Vec<Range> = keys
        .iter()
        .map(|(index, _)| selection.ranges()[*index])
        .collect();
    keys.sort_by(|(_, a), (_, b)| {
        let ordering = a.compare(b);
        if options.reverse {
            ordering.reverse()
        } else {
            ordering
        }
    });
    // Drop borrowed keys before moving fragments into the transaction.
    let sources: Vec<_> = keys.into_iter().map(|(index, _)| index).collect();
    Some(Transaction::change(
        doc,
        destinations.into_iter().zip(sources).map(|(range, index)| {
            (
                range.from(),
                range.to(),
                Some(std::mem::take(&mut fragments[index])),
            )
        }),
    ))
}

enum SortKey<'a> {
    Text(Cow<'a, str>),
    Numeric(Number<'a>),
    Natural(Cow<'a, str>),
    Length(usize),
}

impl<'a> SortKey<'a> {
    fn new(text: &'a str, options: &SortOptions<'_>) -> Self {
        match options.mode {
            SortMode::Numeric => Self::Numeric(Number::prefix(text)),
            SortMode::Length => Self::Length(text.chars().count()),
            SortMode::Text | SortMode::Natural => {
                let text = if options.case_insensitive {
                    Cow::Owned(text.to_lowercase())
                } else {
                    Cow::Borrowed(text)
                };
                if options.mode == SortMode::Natural {
                    Self::Natural(text)
                } else {
                    Self::Text(text)
                }
            }
        }
    }

    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Numeric(a), Self::Numeric(b)) => a.compare(b),
            (Self::Natural(a), Self::Natural(b)) => natural_compare(a, b),
            (Self::Length(a), Self::Length(b)) => a.cmp(b),
            _ => unreachable!("all sort keys use the same mode"),
        }
    }
}

/// Exact decimal prefixes, adapted from Helix #16324. Missing prefixes count as zero.
#[derive(Debug, Clone, Copy)]
struct Number<'a> {
    negative: bool,
    integer: &'a str,
    fraction: &'a str,
}

impl<'a> Number<'a> {
    fn prefix(text: &'a str) -> Self {
        let text = text.trim_start();
        let digits = text.strip_prefix('-').unwrap_or(text);
        let integer_end = digits.bytes().take_while(u8::is_ascii_digit).count();
        let integer = &digits[..integer_end];
        let rest = &digits[integer_end..];
        let fraction = rest.strip_prefix('.').map_or("", |rest| {
            &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()]
        });
        let integer = integer.trim_start_matches('0');
        let fraction = fraction.trim_end_matches('0');
        Self {
            negative: text.starts_with('-') && !(integer.is_empty() && fraction.is_empty()),
            integer,
            fraction,
        }
    }

    fn compare(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => {
                let magnitude = self
                    .integer
                    .len()
                    .cmp(&other.integer.len())
                    .then_with(|| self.integer.cmp(other.integer))
                    .then_with(|| self.fraction.cmp(other.fraction));
                if self.negative {
                    magnitude.reverse()
                } else {
                    magnitude
                }
            }
        }
    }
}

/// Unsigned ASCII digit runs compare by magnitude; leading zeroes do not break ties.
/// Signs and punctuation remain ordinary text. No allocation or integer conversion.
fn natural_compare(mut a: &str, mut b: &str) -> Ordering {
    while !a.is_empty() && !b.is_empty() {
        if a.as_bytes()[0].is_ascii_digit() && b.as_bytes()[0].is_ascii_digit() {
            let a_end = a.bytes().take_while(u8::is_ascii_digit).count();
            let b_end = b.bytes().take_while(u8::is_ascii_digit).count();
            let x = a[..a_end].trim_start_matches('0');
            let y = b[..b_end].trim_start_matches('0');
            let ordering = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if ordering != Ordering::Equal {
                return ordering;
            }
            a = &a[a_end..];
            b = &b[b_end..];
        } else {
            let x = a.chars().next().unwrap();
            let y = b.chars().next().unwrap();
            let ordering = x.cmp(&y);
            if ordering != Ordering::Equal {
                return ordering;
            }
            a = &a[x.len_utf8()..];
            b = &b[y.len_utf8()..];
        }
    }
    a.is_empty().cmp(&b.is_empty()).reverse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(fragments: &[&str], options: &SortOptions<'_>) -> Vec<String> {
        let mut doc = Rope::from(fragments.join("\n"));
        let mut start = 0;
        let ranges = fragments
            .iter()
            .map(|fragment| {
                let end = start + fragment.chars().count();
                let range = Range::new(start, end);
                start = end + 1;
                range
            })
            .collect();
        let selection = Selection::new(ranges, 0);
        let transaction = sort_selection_contents(&doc, &selection, options).unwrap();
        assert!(transaction.apply(&mut doc));
        doc.to_string().split('\n').map(str::to_owned).collect()
    }

    fn options(mode: SortMode) -> SortOptions<'static> {
        SortOptions {
            mode,
            ..SortOptions::default()
        }
    }

    #[test]
    fn numeric_prefixes_handle_signs_decimals_and_missing_numbers() {
        assert_eq!(
            sorted(
                &["10", "2", "-2", "1.5", "text", "+5", "-0", "0", ".5", "-.25", " 3tail"],
                &options(SortMode::Numeric),
            ),
            ["-2", "-.25", "text", "+5", "-0", "0", ".5", "1.5", "2", " 3tail", "10"]
        );
        assert_eq!(
            sorted(&["2", "1e9", "-.", "-.0"], &options(SortMode::Numeric)),
            ["-.", "-.0", "1e9", "2"]
        );
    }

    #[test]
    fn numeric_order_is_exact_beyond_machine_precision() {
        assert_eq!(
            sorted(
                &[
                    "10000000000000000001",
                    "9999999999999999999",
                    "9007199254740993",
                    "9007199254740992",
                    "0.00000000000000000002",
                    "0.00000000000000000001",
                    "-10000000000000000001"
                ],
                &options(SortMode::Numeric),
            ),
            [
                "-10000000000000000001",
                "0.00000000000000000001",
                "0.00000000000000000002",
                "9007199254740992",
                "9007199254740993",
                "9999999999999999999",
                "10000000000000000001"
            ]
        );
    }

    #[test]
    fn equal_numeric_keys_are_stable_in_both_directions() {
        let input = ["1.0 x", "01 y", "1 z", "1.00 w"];
        for reverse in [false, true] {
            assert_eq!(
                sorted(
                    &input,
                    &SortOptions {
                        reverse,
                        ..options(SortMode::Numeric)
                    }
                ),
                input
            );
        }
    }

    #[test]
    fn natural_order_handles_multiple_long_digit_runs_and_leading_zeroes() {
        assert_eq!(
            sorted(
                &[
                    "item10",
                    "item02",
                    "item2",
                    "item1",
                    "item0000000000000000000000003",
                    "item20"
                ],
                &options(SortMode::Natural),
            ),
            [
                "item1",
                "item02",
                "item2",
                "item0000000000000000000000003",
                "item10",
                "item20"
            ]
        );
        assert_eq!(
            sorted(&["v2.10", "v10.1", "v2.2"], &options(SortMode::Natural)),
            ["v2.2", "v2.10", "v10.1"]
        );
        assert_eq!(
            sorted(
                &["É2", "é10", "é2", "E3"],
                &SortOptions {
                    case_insensitive: true,
                    ..options(SortMode::Natural)
                }
            ),
            ["E3", "É2", "é2", "é10"]
        );
        assert_eq!(
            sorted(
                &["x2", "x02", "x10"],
                &SortOptions {
                    reverse: true,
                    ..options(SortMode::Natural)
                }
            ),
            ["x10", "x2", "x02"]
        );
    }

    #[test]
    fn length_counts_original_unicode_scalars_and_keeps_ties_stable() {
        assert_eq!(
            sorted(
                &["界界", "zz", "e\u{301}", "🙂", "long"],
                &options(SortMode::Length)
            ),
            ["🙂", "界界", "zz", "e\u{301}", "long"]
        );
        // Lowercasing İ expands to two scalars, but must not change the length key.
        assert_eq!(
            sorted(
                &["İ", "a", "bb"],
                &SortOptions {
                    case_insensitive: true,
                    reverse: true,
                    ..options(SortMode::Length)
                }
            ),
            ["bb", "İ", "a"]
        );
    }

    #[test]
    fn ignoring_leading_blanks_changes_keys_without_changing_contents() {
        assert_eq!(
            sorted(
                &["  b", "a", "\tA"],
                &SortOptions {
                    ignore_leading_blanks: true,
                    case_insensitive: true,
                    ..SortOptions::default()
                }
            ),
            ["a", "\tA", "  b"]
        );
        assert_eq!(sorted(&["  b", "a"], &SortOptions::default()), ["  b", "a"]);
    }

    #[test]
    fn regex_keys_move_whole_fragments_and_keep_unmatched_slots() {
        let key = Regex::new(r"\d+").unwrap();
        for (reverse, expected) in [
            (false, ["header", "界2", "gap", "z10", "a20"]),
            (true, ["header", "a20", "gap", "z10", "界2"]),
        ] {
            assert_eq!(
                sorted(
                    &["header", "z10", "gap", "界2", "a20"],
                    &SortOptions {
                        key: Some(&key),
                        reverse,
                        ..options(SortMode::Numeric)
                    }
                ),
                expected
            );
        }
        assert_eq!(
            sorted(
                &["header", "gap"],
                &SortOptions {
                    key: Some(&key),
                    ..SortOptions::default()
                }
            ),
            ["header", "gap"]
        );
        let uppercase = Regex::new("[A-Z]+").unwrap();
        assert_eq!(
            sorted(
                &["b", "C", "A"],
                &SortOptions {
                    key: Some(&uppercase),
                    case_insensitive: true,
                    ..SortOptions::default()
                }
            ),
            ["b", "A", "C"]
        );
        let empty = Regex::new("").unwrap();
        assert_eq!(
            sorted(
                &["b", "a"],
                &SortOptions {
                    key: Some(&empty),
                    reverse: true,
                    ..SortOptions::default()
                }
            ),
            ["b", "a"]
        );
    }

    #[test]
    fn sorting_preserves_gaps_selection_directions_primary_and_undo() {
        let (text, selection) = crate::test::print("#(10|)#\r\n gap #[|界2]# / #(a1|)#");
        let original = Rope::from(text);
        let mut doc = original.clone();
        let key = Regex::new(r"\d+").unwrap();
        let transaction = sort_selection_contents(
            &doc,
            &selection,
            &SortOptions {
                key: Some(&key),
                ..options(SortMode::Numeric)
            },
        )
        .unwrap();
        let mapped = selection.clone().map(transaction.changes());
        assert!(transaction.apply(&mut doc));
        let (expected, expected_selection) =
            crate::test::print("#(a1|)#\r\n gap #[|界2]# / #(10|)#");
        assert_eq!(doc, expected.as_str());
        assert_eq!(mapped, expected_selection);
        let undo = transaction.invert(&original);
        assert!(undo.apply(&mut doc));
        assert_eq!(doc, original);
    }

    #[test]
    fn comparators_obey_ordering_laws() {
        let values = [
            "", "0", "00", "-0", "-.2", "-2", "a", "a0", "a00", "a2", "a02", "a10", "a.2", "é2",
            "é10", "1.2", "1.20", "1.21", "1e3", "+2", "界",
        ];
        for mode in [SortMode::Numeric, SortMode::Natural] {
            let options = options(mode);
            let keys: Vec<_> = values
                .iter()
                .map(|text| SortKey::new(text, &options))
                .collect();
            for a in &keys {
                for b in &keys {
                    assert_eq!(a.compare(b), b.compare(a).reverse());
                    for c in &keys {
                        if a.compare(b) != Ordering::Greater && b.compare(c) != Ordering::Greater {
                            assert_ne!(a.compare(c), Ordering::Greater);
                        }
                    }
                }
            }
        }
    }
}
