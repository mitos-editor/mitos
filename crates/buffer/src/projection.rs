//! Editable excerpts over caller-owned sources. All offsets count Unicode scalars.
//!
//! Source identity, loading, protocol synchronization, and writes belong to the
//! caller. A projection partitions its text, including empty, deleted excerpts.

use std::ops::Range;

use editor_core::{Assoc, ChangeSet, Rope};

#[derive(Debug, Clone)]
pub struct Projection<S> {
    pub segments: Vec<Excerpt<S>>,
}

#[derive(Debug, Clone)]
pub struct Excerpt<S> {
    pub source: S,
    pub projection_range: Range<usize>,
    pub source_range: Range<usize>,
    pub source_line_start: usize,
    pub original_text: Rope,
}

impl<S> Projection<S> {
    /// Each boundary has one owner: insertions at a boundary belong to the next
    /// excerpt, except at EOF. Never drop empty excerpts: undo can restore them.
    pub fn apply_changes(&mut self, changes: &ChangeSet) {
        let last = self.segments.len().saturating_sub(1);
        for (index, segment) in self.segments.iter_mut().enumerate() {
            segment.projection_range.start = if index == 0 {
                0
            } else {
                changes.map_pos(segment.projection_range.start, Assoc::Before)
            };
            segment.projection_range.end = if index == last {
                changes.map_pos(changes.len_before(), Assoc::After)
            } else {
                changes.map_pos(segment.projection_range.end, Assoc::Before)
            };
        }
    }

    pub fn segment_at(&self, pos: usize) -> Option<usize> {
        self.segments
            .iter()
            .position(|s| s.projection_range.contains(&pos))
            .or_else(|| {
                self.segments
                    .iter()
                    .rposition(|s| s.projection_range.end == pos)
            })
    }

    pub fn is_modified(&self, text: &Rope) -> bool {
        self.segments
            .iter()
            .any(|s| text.slice(s.projection_range.clone()) != s.original_text.slice(..))
    }

    pub fn is_valid(&self, text: &Rope) -> bool {
        let mut end = 0;
        for segment in &self.segments {
            if segment.projection_range.start != end || segment.projection_range.end < end {
                return false;
            }
            end = segment.projection_range.end;
        }
        !self.segments.is_empty()
            && end == text.len_chars()
            && self.segments.iter().all(|s| {
                editor_core::graphemes::ensure_grapheme_boundary_prev(
                    text.slice(..),
                    s.projection_range.start,
                ) == s.projection_range.start
            })
    }
}

impl<S> Excerpt<S> {
    pub fn projection_line_range(&self, text: &Rope) -> Range<usize> {
        let start = text.char_to_line(self.projection_range.start);
        let end = text.char_to_line(
            self.projection_range
                .end
                .saturating_sub(1)
                .max(self.projection_range.start),
        );
        start..end + 1
    }

    pub fn source_line_for_projection_line(&self, text: &Rope, line: usize) -> Option<usize> {
        let lines = self.projection_line_range(text);
        lines
            .contains(&line)
            .then(|| self.source_line_start + line - lines.start)
    }

    pub fn last_source_line(&self, text: &Rope) -> Option<usize> {
        self.source_line_for_projection_line(text, self.projection_line_range(text).end - 1)
    }
}

/// Expand inclusive location lines with context, rejecting stale and reversed locations.
pub fn excerpt_lines(
    text: &Rope,
    range: Option<(usize, usize)>,
    context: usize,
) -> Option<Range<usize>> {
    let last = text.len_lines() - 1;
    let (start, end) = range.unwrap_or((0, context.saturating_mul(2)));
    if start > last || end < start {
        return None;
    }
    Some(start.saturating_sub(context)..end.saturating_add(context).min(last) + 1)
}

pub fn merge_line_ranges(ranges: &mut Vec<Range<usize>>) {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    *ranges = merged;
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::Transaction;

    fn projection() -> Projection<usize> {
        Projection {
            segments: vec![
                Excerpt {
                    source: 1,
                    projection_range: 0..2,
                    source_range: 0..2,
                    source_line_start: 0,
                    original_text: "é\n".into(),
                },
                Excerpt {
                    source: 2,
                    projection_range: 2..4,
                    source_range: 0..2,
                    source_line_start: 0,
                    original_text: "β\n".into(),
                },
            ],
        }
    }

    #[test]
    fn insertions_at_shared_boundaries_have_one_owner() {
        let mut text = Rope::from_str("é\nβ\n");
        let mut projection = projection();
        for (pos, inserted) in [(2, "🦀"), (0, "x"), (6, "!")] {
            let tx = Transaction::change(&text, [(pos, pos, Some(inserted.into()))].into_iter());
            projection.apply_changes(tx.changes());
            assert!(tx.changes().apply(&mut text));
            assert!(projection.is_valid(&text));
        }
        assert_eq!(projection.segments[0].projection_range, 0..3);
        assert_eq!(projection.segments[1].projection_range, 3..7);
    }

    #[test]
    fn cross_excerpt_deletion_preserves_empty_sources() {
        let mut projection = projection();
        let mut text = Rope::from_str("é\nβ\n");
        let tx = Transaction::change(&text, [(0, 4, None)].into_iter());
        projection.apply_changes(tx.changes());
        tx.changes().apply(&mut text);
        assert!(projection.is_valid(&text));
        assert_eq!(projection.segments.len(), 2);
        assert!(projection
            .segments
            .iter()
            .all(|s| s.projection_range == (0..0)));
        assert!(projection.is_modified(&text));
    }

    #[test]
    fn location_ranges_are_bounded_and_merged() {
        let text = Rope::from_str("a\nb\nc\nd\n");
        assert_eq!(excerpt_lines(&text, Some((3, usize::MAX)), 3), Some(0..5));
        assert_eq!(excerpt_lines(&text, Some((2, 1)), 3), None);
        assert_eq!(excerpt_lines(&text, Some((99, 99)), 3), None);
        let mut ranges = vec![3..5, 0..2, 2..4, 9..10];
        merge_line_ranges(&mut ranges);
        assert_eq!(ranges, [0..5, 9..10]);
    }
}
