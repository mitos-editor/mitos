//! Half-open ranges and ordered range-set helpers.

use std::ops::{self, RangeBounds};

/// A half-open range from `start` (inclusive) to `end` (exclusive).
///
/// With the default `usize` parameter this usually stores rope character
/// indices. The generic form is also used for other ordered coordinate types.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub struct Range<T = usize> {
    pub start: T,
    pub end: T,
}

impl<T: PartialOrd> Range<T> {
    /// Returns whether `other` is entirely contained by this range.
    pub fn contains(&self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
    /// Returns whether the end does not follow the start.
    ///
    /// Inverted ranges are considered empty as well as zero-width ranges.
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

impl<T> RangeBounds<T> for Range<T> {
    fn start_bound(&self) -> ops::Bound<&T> {
        ops::Bound::Included(&self.start)
    }

    fn end_bound(&self) -> ops::Bound<&T> {
        ops::Bound::Excluded(&self.end)
    }
}

/// Returns true if all ranges yielded by `sub_set` are contained by
/// `super_set`. This is essentially an optimized implementation of
/// `sub_set.all(|rb| super_set.any(|ra| ra.contains(rb)))` that runs in O(m+n)
/// instead of O(mn) (and in many cases faster).
///
/// Both iterators must uphold the following invariants:
///
/// - ranges must not overlap (but they can be adjacent);
/// - ranges must be sorted.
pub fn is_subset<const ALLOW_EMPTY: bool>(
    mut super_set: impl Iterator<Item = Range>,
    mut sub_set: impl Iterator<Item = Range>,
) -> bool {
    let (mut super_range, mut sub_range) = (super_set.next(), sub_set.next());
    loop {
        match (super_range, sub_range) {
            // skip over irrelevant ranges
            (Some(ra), Some(rb))
                if ra.end <= rb.start && (ra.start != rb.start || !ALLOW_EMPTY) =>
            {
                super_range = super_set.next();
            }
            (Some(ra), Some(rb)) => {
                if ra.contains(rb) {
                    sub_range = sub_set.next();
                } else {
                    return false;
                }
            }
            (None, Some(_)) => {
                // exhausted `super_set`, we can't match the reminder of `sub_set`
                return false;
            }
            (_, None) => {
                // no elements from `sub_sut` left to match, `super_set` contains `sub_set`
                return true;
            }
        }
    }
}

/// Like [`is_subset`], but requires each element of `super_set` to contain at
/// least one element of `sub_set`.
pub fn is_exact_subset(
    mut super_set: impl Iterator<Item = Range>,
    mut sub_set: impl Iterator<Item = Range>,
) -> bool {
    let (mut super_range, mut sub_range) = (super_set.next(), sub_set.next());
    let mut super_range_matched = false;
    loop {
        match (super_range, sub_range) {
            // skip over irrelevant ranges
            (Some(ra), Some(rb)) if ra.end <= rb.start && ra.start < rb.start => {
                if !super_range_matched {
                    return false;
                }
                super_range_matched = false;
                super_range = super_set.next();
            }
            (Some(ra), Some(rb)) => {
                if ra.contains(rb) {
                    super_range_matched = true;
                    sub_range = sub_set.next();
                } else {
                    return false;
                }
            }
            (None, Some(_)) => {
                // exhausted `super_set`, we can't match the reminder of `sub_set`
                return false;
            }
            (_, None) => {
                // Every outer range, including the current one, must have a match.
                return (super_range.is_none() || super_range_matched)
                    && super_set.next().is_none();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_exact_subset, Range};

    #[test]
    fn exact_subset_rejects_an_uncovered_first_range() {
        assert!(!is_exact_subset(
            [Range { start: 0, end: 10 }, Range { start: 20, end: 30 }].into_iter(),
            [Range { start: 21, end: 22 }, Range { start: 23, end: 24 }].into_iter(),
        ));
    }

    #[test]
    fn exact_subset_requires_every_outer_range_to_be_matched() {
        let cases: &[(&[_], &[_], bool)] = &[
            (&[], &[], true),
            (&[], &[(1, 2)], false),
            (&[(0, 10)], &[], false),
            (&[(0, 10), (20, 30)], &[], false),
            (&[(0, 10), (20, 30)], &[(21, 22), (23, 24)], false),
            (&[(0, 10), (20, 30), (40, 50)], &[(1, 2), (41, 42)], false),
            (&[(0, 10), (20, 30)], &[(1, 2)], false),
            (&[(0, 10)], &[(1, 2)], true),
            (&[(0, 10), (20, 30)], &[(1, 2), (21, 22)], true),
            (&[(0, 10)], &[(1, 2), (3, 4)], true),
            (&[(0, 10), (20, 30)], &[(1, 2), (3, 4), (21, 22)], true),
            (&[(0, 10)], &[(1, 11)], false),
            (&[(0, 10)], &[(11, 12)], false),
            (&[(0, 10), (20, 30)], &[(1, 2), (11, 12), (21, 22)], false),
        ];
        for &(outer, inner, expected) in cases {
            let ranges = |&(start, end)| Range { start, end };
            assert_eq!(
                is_exact_subset(outer.iter().map(ranges), inner.iter().map(ranges)),
                expected,
                "outer: {outer:?}, inner: {inner:?}",
            );
        }
    }

    #[test]
    fn exact_subset_preserves_adjacent_and_zero_width_boundaries() {
        let cases: &[(&[_], &[_], bool)] = &[
            (&[(0, 10), (10, 20)], &[(0, 10), (10, 20)], true),
            (&[(0, 10), (10, 20)], &[(0, 0), (10, 10)], true),
            (&[(0, 10), (10, 20)], &[(10, 10), (11, 12)], false),
            (&[(0, 10)], &[(10, 10)], false),
            (&[(0, 0)], &[(0, 0)], true),
            (&[(0, 0), (0, 10)], &[(0, 0), (1, 2)], true),
            (&[(0, 10), (10, 10)], &[(1, 2), (10, 10)], true),
        ];
        for &(outer, inner, expected) in cases {
            let ranges = |&(start, end)| Range { start, end };
            assert_eq!(
                is_exact_subset(outer.iter().map(ranges), inner.iter().map(ranges)),
                expected,
                "outer: {outer:?}, inner: {inner:?}",
            );
        }
    }
}
