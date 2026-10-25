//! Optional, bounded statistics over the canonical full-document match stream.
//! Navigation retains its own windowed matching semantics.

use crate::{Range, RopeSlice};
use stdx::rope::{self, RopeSliceExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    Exact(usize),
    MoreThan(usize),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub current: Count,
    pub total: Count,
}

impl Position {
    pub const PENDING: Self = Self {
        current: Count::Unknown,
        total: Count::Unknown,
    };
}

#[derive(Debug)]
pub struct Statistics {
    matches: Vec<Range>,
    total: Count,
}

impl Statistics {
    /// Exact ordinals require membership in the canonical stream. Windowed
    /// searches can overlap or truncate its matches, so absence is ordinary.
    /// Beyond a capped prefix, only a position past that prefix is known.
    pub fn position(&self, range: Range) -> Position {
        let current = match self
            .matches
            .binary_search_by_key(&range.from(), |r| r.from())
        {
            Ok(index) if self.matches[index].to() == range.to() => match self.total {
                Count::MoreThan(limit) if index >= limit => Count::MoreThan(limit),
                _ => Count::Exact(index + 1),
            },
            Err(index) if index == self.matches.len() => match self.total {
                Count::MoreThan(limit) => Count::MoreThan(limit),
                _ => Count::Unknown,
            },
            _ => Count::Unknown,
        };
        Position {
            current,
            total: self.total,
        }
    }
}

/// Retain at most `limit + 1` matches. The extra match proves the cap was
/// exceeded. `stop` is checked around each regex step, including EOF; it cannot
/// interrupt an individual regex call. Callers must run this off the UI thread.
pub fn count_regex(
    text: RopeSlice<'_>,
    regex: &rope::Regex,
    limit: Option<usize>,
    mut stop: impl FnMut() -> bool,
) -> Statistics {
    let mut result = Statistics {
        matches: Vec::new(),
        total: Count::Unknown,
    };
    let mut matches = regex.find_iter(text.regex_input());
    loop {
        if stop() {
            return result;
        }
        let matched = matches.next();
        if stop() {
            return result;
        }
        let Some(matched) = matched else {
            result.total = Count::Exact(result.matches.len());
            return result;
        };
        result.matches.push(Range::new(
            text.byte_to_char_idx(matched.start()),
            text.byte_to_char_idx(matched.end()),
        ));
        if let Some(limit) = limit
            && result.matches.len() > limit
        {
            result.total = Count::MoreThan(limit);
            return result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{movement::Direction, search::find_regex, Rope};

    #[test]
    fn capped_statistics_retain_only_the_prefix_and_one_proof_match() {
        let text = Rope::from("one ".repeat(100_000));
        let regex = rope::Regex::new("one").unwrap();
        let stats = count_regex(text.slice(..), &regex, Some(2), || false);
        assert_eq!(stats.matches.len(), 3);
        assert_eq!(
            stats.position(Range::new(4, 7)),
            Position {
                current: Count::Exact(2),
                total: Count::MoreThan(2)
            }
        );
        for range in [Range::new(8, 11), Range::new(399_996, 399_999)] {
            assert_eq!(
                stats.position(range),
                Position {
                    current: Count::MoreThan(2),
                    total: Count::MoreThan(2)
                }
            );
        }
    }

    #[test]
    fn exact_totals_include_empty_documents_exact_limits_and_unlimited_counts() {
        let regex = rope::Regex::new("one").unwrap();
        for (text, limit, count) in [
            ("", Some(2), 0),
            ("one one", Some(2), 2),
            ("one one one", None, 3),
        ] {
            let text = Rope::from(text);
            let stats = count_regex(text.slice(..), &regex, limit, || false);
            assert_eq!(stats.total, Count::Exact(count));
        }
        let text = Rope::from("one");
        let stats = count_regex(text.slice(..), &regex, Some(0), || false);
        assert_eq!(
            stats.position(Range::new(0, 3)),
            Position {
                current: Count::MoreThan(0),
                total: Count::MoreThan(0)
            }
        );
    }

    #[test]
    fn windowed_matches_do_not_need_membership_in_the_canonical_stream() {
        let text = Rope::from("aaa");
        for (pattern, start, direction, expected) in [
            ("aa", Range::new(0, 1), Direction::Forward, Range::new(1, 3)),
            (
                "a+",
                Range::new(2, 3),
                Direction::Backward,
                Range::new(0, 2),
            ),
        ] {
            let regex = rope::Regex::new(pattern).unwrap();
            let found = find_regex(text.slice(..), start, &regex, direction, false).unwrap();
            assert_eq!(found.range, expected);
            let stats = count_regex(text.slice(..), &regex, Some(100), || false);
            assert_eq!(
                stats.position(found.range),
                Position {
                    current: Count::Unknown,
                    total: Count::Exact(1)
                }
            );
        }
    }

    #[test]
    fn unicode_crlf_multiline_and_zero_width_matches_keep_character_coordinates() {
        let text = Rope::from("é\r\n界\r\n");
        let regex = rope::RegexBuilder::new()
            .syntax(rope::Config::new().multi_line(true).crlf(true))
            .build("^|界")
            .unwrap();
        let stats = count_regex(text.slice(..), &regex, None, || false);
        assert_eq!(
            stats.matches,
            vec![Range::point(0), Range::point(3), Range::point(6)]
        );
        assert_eq!(stats.position(Range::point(6)).current, Count::Exact(3));
        let regex = rope::Regex::new("界").unwrap();
        let stats = count_regex(text.slice(..), &regex, None, || false);
        assert_eq!(
            stats.position(Range::new(4, 3)),
            Position {
                current: Count::Exact(1),
                total: Count::Exact(1)
            }
        );
    }

    #[test]
    fn interrupted_scans_never_report_an_exact_total_even_after_eof() {
        let text = Rope::from("one one");
        let regex = rope::Regex::new("one").unwrap();
        let mut checks = 0;
        let stats = count_regex(text.slice(..), &regex, None, || {
            checks += 1;
            checks >= 3
        });
        assert_eq!(stats.matches.len(), 1);
        assert_eq!(
            stats.position(Range::new(0, 3)),
            Position {
                current: Count::Exact(1),
                total: Count::Unknown
            }
        );
        let text = Rope::from("no match");
        let mut checks = 0;
        let stats = count_regex(text.slice(..), &regex, None, || {
            checks += 1;
            checks >= 2
        });
        assert_eq!(stats.total, Count::Unknown);
        assert!(stats.matches.is_empty());
    }
}
