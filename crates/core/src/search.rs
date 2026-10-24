use crate::movement::Direction;
use crate::{Range, RopeSlice, Selection};
use stdx::rope::RopeExt as _;
use stdx::rope::{self, RopeSliceExt};

/// A buffer match in character coordinates and whether reaching it wrapped the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub range: Range,
    pub wrapped: bool,
}

/// Find the next/previous regex match after the primary selection. Search windows
/// retain full-rope context for anchors. A point at EOF cannot repeatedly match itself.
pub fn find_regex(
    text: RopeSlice,
    primary: Range,
    regex: &rope::Regex,
    direction: Direction,
    wrap: bool,
) -> Option<SearchMatch> {
    let boundary = match direction {
        Direction::Forward => primary.to(),
        Direction::Backward => primary.from(),
    };
    let start = text.char_to_byte_idx(boundary);
    let find =
        |wrapped| match (direction, wrapped) {
            (Direction::Forward, false) => regex
                .find_iter(text.regex_input_at_bytes(start..))
                .find(|matched| {
                    !(matched.is_empty() && matched.start() == start && primary.is_empty())
                }),
            (Direction::Backward, false) => regex
                .find_iter(text.regex_input_at_bytes(..start))
                .filter(|matched| !(matched.is_empty() && matched.start() == start))
                .last(),
            (Direction::Forward, true) => regex.find_iter(text.regex_input()).find(|matched| {
                !(matched.is_empty() && matched.start() == start && primary.is_empty())
            }),
            (Direction::Backward, true) => regex
                .find_iter(text.regex_input_at_bytes(start..))
                .filter(|matched| !(matched.is_empty() && matched.start() == start))
                .last(),
        };
    let (matched, wrapped) = find(false).map(|matched| (matched, false)).or_else(|| {
        wrap.then(|| find(true))
            .flatten()
            .map(|matched| (matched, true))
    })?;
    Some(SearchMatch {
        range: Range::new(
            text.byte_to_char_idx(matched.start()),
            text.byte_to_char_idx(matched.end()),
        )
        .with_direction(primary.direction()),
        wrapped,
    })
}

/// Build a literal alternation from selections, deduplicated in stable order.
/// Optional word boundaries follow the surrounding document characters.
pub fn selection_pattern(text: RopeSlice, selection: &Selection, word_boundaries: bool) -> String {
    use crate::chars::char_is_word;
    let word_start = |index| {
        index < text.len_chars()
            && char_is_word(text.char(index))
            && (index == 0 || !char_is_word(text.char(index - 1)))
    };
    let word_end = |index| {
        index > 0
            && index < text.len_chars()
            && char_is_word(text.char(index - 1))
            && !char_is_word(text.char(index))
    };
    selection
        .iter()
        .map(|range| {
            let prefix = if word_boundaries && word_start(range.from()) {
                "\\b"
            } else {
                ""
            };
            let suffix = if word_boundaries && word_end(range.to()) {
                "\\b"
            } else {
                ""
            };
            format!(
                "{prefix}{}{suffix}",
                crate::regex::escape(&range.fragment(text))
            )
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("|")
}

// TODO: switch to std::str::Pattern when it is stable.
pub trait CharMatcher {
    fn char_match(&self, ch: char) -> bool;
}

impl CharMatcher for char {
    fn char_match(&self, ch: char) -> bool {
        *self == ch
    }
}

impl<F: Fn(&char) -> bool> CharMatcher for F {
    fn char_match(&self, ch: char) -> bool {
        (*self)(&ch)
    }
}

// Finds the positions of the nth matching character in given direction
// starting from the pos gap-index (see Range struct for explanation)
pub fn find_nth_char<M: CharMatcher>(
    mut n: usize,
    text: RopeSlice,
    char_matcher: M,
    mut pos: usize,
    direction: Direction,
) -> Option<usize> {
    if n == 0 {
        return None;
    }

    let mut chars = text.get_chars_at_char(pos)?;

    match direction {
        Direction::Forward => loop {
            let c = chars.next()?;
            if char_matcher.char_match(c) {
                n -= 1;
                if n == 0 {
                    return Some(pos);
                }
            }
            pos += 1;
        },
        Direction::Backward => loop {
            let c = chars.prev()?;
            pos -= 1;
            if char_matcher.char_match(c) {
                n -= 1;
                if n == 0 {
                    return Some(pos);
                }
            }
        },
    };
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::movement::Direction;

    #[test]
    fn test_find_nth_char() {
        let text = RopeSlice::from("aa ⌚aa \r\n aa");

        // Forward direction
        assert_eq!(find_nth_char(1, text, 'a', 5, Direction::Forward), Some(5));
        assert_eq!(find_nth_char(2, text, 'a', 5, Direction::Forward), Some(10));
        assert_eq!(find_nth_char(3, text, 'a', 5, Direction::Forward), Some(11));
        assert_eq!(find_nth_char(4, text, 'a', 5, Direction::Forward), None);

        // Backward direction
        assert_eq!(find_nth_char(1, text, 'a', 5, Direction::Backward), Some(4));
        assert_eq!(find_nth_char(2, text, 'a', 5, Direction::Backward), Some(1));
        assert_eq!(find_nth_char(3, text, 'a', 5, Direction::Backward), Some(0));
        assert_eq!(find_nth_char(4, text, 'a', 5, Direction::Backward), None);

        // Edge cases
        assert_eq!(find_nth_char(0, text, 'a', 5, Direction::Forward), None); // n = 0
        assert_eq!(find_nth_char(1, text, 'x', 5, Direction::Forward), None); // Not found
        assert_eq!(find_nth_char(1, text, 'a', 20, Direction::Forward), None); // Beyond text
        assert_eq!(find_nth_char(1, text, 'a', 0, Direction::Backward), None); // At start going backward
    }
}

#[cfg(test)]
mod buffer_tests {
    use super::*;
    use crate::Rope;

    #[test]
    fn regex_search_uses_character_coordinates_and_wraps_in_both_directions() {
        let text = Rope::from("é😀é\n");
        let text = text.slice(..);
        let regex = rope::Regex::new("é").unwrap();
        let first = Range::new(0, 1);
        let last = Range::new(2, 3);
        assert_eq!(
            find_regex(text, first, &regex, Direction::Forward, false),
            Some(SearchMatch {
                range: last,
                wrapped: false
            })
        );
        assert_eq!(
            find_regex(text, last, &regex, Direction::Forward, false),
            None
        );
        assert_eq!(
            find_regex(text, last, &regex, Direction::Forward, true),
            Some(SearchMatch {
                range: first,
                wrapped: true
            })
        );
        assert_eq!(
            find_regex(text, first, &regex, Direction::Backward, true),
            Some(SearchMatch {
                range: last,
                wrapped: true
            })
        );
        let backward =
            find_regex(text, Range::new(3, 2), &regex, Direction::Backward, false).unwrap();
        assert_eq!(backward.range, Range::new(1, 0));
    }

    #[test]
    fn zero_width_matches_advance_past_document_edges() {
        let text = Rope::from("a\n");
        let text = text.slice(..);
        let regex = rope::Regex::new("(?m)$").unwrap();
        let eof = find_regex(text, Range::new(1, 2), &regex, Direction::Forward, false).unwrap();
        assert_eq!(eof.range, Range::point(2));
        assert!(find_regex(text, eof.range, &regex, Direction::Forward, false).is_none());
        let wrapped = find_regex(text, eof.range, &regex, Direction::Forward, true).unwrap();
        assert_eq!(wrapped.range, Range::point(1));
        assert!(wrapped.wrapped);
        let start = rope::Regex::new("(?m)^").unwrap();
        let wrapped =
            find_regex(text, Range::new(0, 1), &start, Direction::Backward, true).unwrap();
        assert_eq!(wrapped.range, Range::point(2));
    }

    #[test]
    fn search_windows_preserve_line_anchors_for_lf_and_crlf() {
        for newline in ["\n", "\r\n"] {
            let text = Rope::from(format!("a{newline}b{newline}c"));
            let text = text.slice(..);
            let regex = rope::RegexBuilder::new()
                .syntax(rope::Config::new().multi_line(true).crlf(newline == "\r\n"))
                .build("^b$")
                .unwrap();
            let found =
                find_regex(text, Range::new(0, 1), &regex, Direction::Forward, false).unwrap();
            assert_eq!(
                found.range,
                Range::new(1 + newline.len(), 2 + newline.len())
            );
            assert_eq!(
                find_regex(
                    text,
                    Range::point(text.len_chars()),
                    &regex,
                    Direction::Backward,
                    false
                )
                .unwrap()
                .range,
                found.range
            );
        }
    }

    #[test]
    fn selection_patterns_escape_literals_deduplicate_and_have_stable_order() {
        let text = Rope::from("beta alpha beta\n");
        let selection = Selection::new(
            vec![Range::new(0, 4), Range::new(5, 10), Range::new(11, 15)].into(),
            1,
        );
        assert_eq!(
            selection_pattern(text.slice(..), &selection, true),
            r"\balpha\b|\bbeta\b"
        );
        let text = Rope::from("é.+");
        assert_eq!(
            selection_pattern(text.slice(..), &Selection::single(0, 3), false),
            r"é\.\+"
        );
        assert_eq!(
            selection_pattern(text.slice(..), &Selection::point(3), true),
            ""
        );
    }
}

/// Add every match while preserving existing ranges, their primary selection, and its direction.
pub fn extend_all_matches(
    text: RopeSlice,
    selection: &Selection,
    regex: &rope::Regex,
) -> Option<Selection> {
    let direction = selection.primary().direction();
    let mut ranges: crate::SmallVec<[Range; 1]> = selection.ranges().into();
    let original_len = ranges.len();
    let mut bounds: std::collections::BTreeSet<_> = ranges
        .iter()
        .map(|range| (range.from(), range.to()))
        .collect();
    for mat in regex.find_iter(text.regex_input()) {
        let range = Range::new(
            text.byte_to_char_idx(mat.start()),
            text.byte_to_char_idx(mat.end()),
        )
        .with_direction(direction);
        if bounds.insert((range.from(), range.to())) {
            ranges.push(range);
        }
    }
    (ranges.len() > original_len).then(|| Selection::new(ranges, selection.primary_index()))
}

#[cfg(test)]
mod extend_all_tests {
    use super::*;
    #[test]
    fn extend_all_preserves_primary_direction_and_handles_missing_and_empty_matches() {
        let text: RopeSlice = "cat dog cat cat".into();
        let original = Selection::single(11, 8);
        let result =
            extend_all_matches(text, &original, &rope::Regex::new("cat").unwrap()).unwrap();
        assert_eq!(result.primary(), original.primary());
        assert_eq!(
            result.ranges(),
            &[Range::new(3, 0), Range::new(11, 8), Range::new(15, 12)]
        );
        assert!(
            extend_all_matches(text, &original, &rope::Regex::new("missing").unwrap()).is_none()
        );
        assert_eq!(
            extend_all_matches(text, &original, &rope::Regex::new("$").unwrap())
                .unwrap()
                .len(),
            2
        );
    }
}
