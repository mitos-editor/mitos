//! Unified, source-mapped review rows independent of VCS and frontend rendering.

use std::ops::Range;

use editor_core::{line_ending::get_line_ending, Rope};
use stdx::rope::LINE_TYPE;

use crate::syntax::{self, Changes};
use crate::{Hunk, LineDiff};
use tree_house::tree_sitter::Tree;

pub const MAX_REVIEW_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_REVIEW_LINES: usize = 200_000;

const CONTEXT: usize = 3;
/// Bound the rendered model independently of the size of its source snapshots.
const MAX_ROWS: usize = 20_000;
const MAX_ROW_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewLineKind {
    Context,
    Removed,
    Added,
    Header,
    Notice,
}

#[derive(Clone, Debug)]
pub struct ReviewLine {
    pub kind: ReviewLineKind,
    /// Zero-based source lines. Headers and notices have neither.
    pub before: Option<usize>,
    pub after: Option<usize>,
    pub text: String,
    /// Changed UTF-8 byte ranges in `text`. `None` uses whole-line emphasis.
    pub changes: Option<Vec<Range<usize>>>,
}

/// An immutable review snapshot. Text is literal and never parsed as Markdown.
#[derive(Clone, Debug, Default)]
pub struct Review {
    pub lines: Vec<ReviewLine>,
    pub additions: usize,
    pub deletions: usize,
    pub truncated: bool,
    /// All syntax tokens matched despite exact text changes.
    pub layout_only: bool,
    selected_hunk: Option<Hunk>,
    bytes: usize,
    largest_line: usize,
    max_line_chars: usize,
}

impl Review {
    pub fn notice(message: &str) -> Self {
        let mut review = Self::default();
        review.push(ReviewLineKind::Notice, None, None, message.into());
        review
    }

    /// Review all hunks, or only the hunk containing the given current line.
    pub fn new(base: Rope, current: Rope, line: Option<usize>) -> Self {
        if line_count(&base).max(line_count(&current)) > MAX_REVIEW_LINES
            || base.len().max(current.len()) > MAX_REVIEW_BYTES
        {
            let mut review = Self::default();
            review.push(
                ReviewLineKind::Notice,
                None,
                None,
                "Text too large to review (limit 10 MiB or 200,000 lines)".into(),
            );
            review.truncated = true;
            return review;
        }
        let diff = LineDiff::new(base.clone(), current.clone());
        let hunks: Vec<_> = diff.hunks().collect();
        let selected = line.and_then(|line| {
            hunks.iter().position(|hunk| {
                let start = hunk.after.start as usize;
                let end = hunk.after.end as usize;
                if start == end {
                    line == start
                } else {
                    (start..end).contains(&line)
                }
            })
        });
        let mut review = Self::default();
        if hunks.is_empty() || (line.is_some() && selected.is_none()) {
            review.push(ReviewLineKind::Notice, None, None, "No text changes".into());
            return review;
        }
        let before_lines = line_count(&base);
        let after_lines = line_count(&current);
        let mut group = 0;
        while group < hunks.len() && !review.truncated {
            let mut end = group + 1;
            while end < hunks.len()
                && hunks[end].after.start as usize
                    <= hunks[end - 1].after.end as usize + CONTEXT * 2
            {
                end += 1;
            }
            if selected.is_some_and(|selected| !(group..end).contains(&selected)) {
                group = end;
                continue;
            }
            let first = &hunks[group];
            let last = &hunks[end - 1];
            let leading = CONTEXT
                .min(first.before.start as usize)
                .min(first.after.start as usize);
            let trailing = CONTEXT
                .min(before_lines.saturating_sub(last.before.end as usize))
                .min(after_lines.saturating_sub(last.after.end as usize));
            let before = first.before.start as usize - leading..last.before.end as usize + trailing;
            let after = first.after.start as usize - leading..last.after.end as usize + trailing;
            review.push(
                ReviewLineKind::Header,
                None,
                None,
                format!("@@ -{} +{} @@", range_label(&before), range_label(&after)),
            );
            review.group(
                &base,
                &current,
                &hunks[group..end],
                before.start,
                after.start,
                after.end,
            );
            group = end;
        }
        if review.truncated {
            review.lines.push(ReviewLine {
                kind: ReviewLineKind::Notice,
                before: None,
                after: None,
                text: "Diff truncated: review a smaller change".into(),
                changes: None,
            });
        } else if let Some(selected) = selected {
            review.selected_hunk = Some(hunks[selected].clone());
        }
        review
    }

    /// The exact hunk selected by the requested line, excluding neighboring context hunks.
    /// Full-file, unchanged, and truncated reviews have no selected hunk.
    pub fn selected_hunk(&self) -> Option<&Hunk> {
        self.selected_hunk.as_ref()
    }

    /// Refine changed rows using the trees for these exact source snapshots.
    /// Missing, incompatible, invalid, or oversized syntax leaves textual emphasis intact.
    pub fn with_syntax(
        mut self,
        base: &Rope,
        current: &Rope,
        before: &Tree,
        after: &Tree,
        canceled: &dyn Fn() -> bool,
    ) -> Self {
        if !self.truncated
            && (self.additions != 0 || self.deletions != 0)
            && before.root_node().grammar() == after.root_node().grammar()
            && let Some(changes) = syntax::compare(base, current, before, after, canceled)
        {
            self.annotate(base, current, changes);
        }
        self
    }

    fn annotate(&mut self, base: &Rope, current: &Rope, changes: Changes) {
        self.layout_only = changes.before.is_empty() && changes.after.is_empty();
        for line in &mut self.lines {
            let (rope, row, ranges) = match line.kind {
                ReviewLineKind::Removed => (base, line.before.unwrap(), &changes.before),
                ReviewLineKind::Added => (current, line.after.unwrap(), &changes.after),
                _ => continue,
            };
            let start = rope.line_to_byte_idx(row, LINE_TYPE);
            let end = start + line.text.len();
            let first = ranges.partition_point(|range| range.end <= start);
            line.changes = Some(
                ranges[first..]
                    .iter()
                    .take_while(|range| range.start < end)
                    .map(|range| range.start.max(start) - start..range.end.min(end) - start)
                    .filter(|range| !range.is_empty())
                    .collect(),
            );
        }
    }

    pub fn max_line_chars(&self) -> usize {
        self.max_line_chars
    }

    pub fn line_number_width(&self) -> usize {
        self.largest_line.to_string().len().max(1)
    }

    /// Map a review row to the after-side. Removed rows point at the deletion
    /// boundary, never at an unrelated later hunk. The caller clamps EOF.
    pub fn after_line_at(&self, row: usize) -> Option<usize> {
        let line = self.lines.get(row)?;
        if let Some(after) = line.after {
            return Some(after);
        }
        if line.kind == ReviewLineKind::Notice {
            return None;
        }
        self.lines[row + 1..]
            .iter()
            .take_while(|line| line.kind != ReviewLineKind::Header)
            .find_map(|line| line.after)
            .or_else(|| {
                self.lines[..row]
                    .iter()
                    .rev()
                    .take_while(|line| line.kind != ReviewLineKind::Header)
                    .find_map(|line| line.after.map(|line| line + 1))
            })
            .or_else(|| (line.kind == ReviewLineKind::Removed).then_some(0))
    }

    fn push(
        &mut self,
        kind: ReviewLineKind,
        before: Option<usize>,
        after: Option<usize>,
        text: String,
    ) {
        if self.lines.len() >= MAX_ROWS || self.bytes.saturating_add(text.len()) > MAX_ROW_BYTES {
            self.truncated = true;
            return;
        }
        self.bytes += text.len();
        self.max_line_chars = self.max_line_chars.max(text.chars().count());
        self.largest_line = self.largest_line.max(
            before
                .into_iter()
                .chain(after)
                .max()
                .map_or(0, |line| line + 1),
        );
        self.lines.push(ReviewLine {
            kind,
            before,
            after,
            text,
            changes: None,
        });
    }

    fn source(
        &mut self,
        kind: ReviewLineKind,
        rope: &Rope,
        before: Option<usize>,
        after: Option<usize>,
    ) {
        let line = before
            .filter(|_| kind == ReviewLineKind::Removed)
            .or(after)
            .unwrap();
        if line >= line_count(rope) {
            return;
        }
        let text = rope.line(line, LINE_TYPE);
        if text.len() > MAX_ROW_BYTES.saturating_sub(self.bytes) {
            self.truncated = true;
            return;
        }
        let ending = get_line_ending(&text);
        let mut text = text.to_string();
        text.truncate(text.len() - ending.map_or(0, |ending| ending.as_str().len()));
        self.push(kind, before, after, text);
        if kind != ReviewLineKind::Context && ending.is_none() {
            self.push(
                ReviewLineKind::Notice,
                None,
                None,
                "\\ No newline at end of file".into(),
            );
        }
    }

    fn group(
        &mut self,
        base: &Rope,
        current: &Rope,
        hunks: &[Hunk],
        mut before: usize,
        mut after: usize,
        end: usize,
    ) {
        for hunk in hunks {
            while after < hunk.after.start as usize && !self.truncated {
                self.source(ReviewLineKind::Context, current, Some(before), Some(after));
                before += 1;
                after += 1;
            }
            for line in hunk.before.start as usize..hunk.before.end as usize {
                if self.truncated {
                    return;
                }
                self.deletions += usize::from(line < line_count(base));
                self.source(ReviewLineKind::Removed, base, Some(line), None);
            }
            for line in hunk.after.start as usize..hunk.after.end as usize {
                if self.truncated {
                    return;
                }
                self.additions += usize::from(line < line_count(current));
                self.source(ReviewLineKind::Added, current, None, Some(line));
            }
            before = hunk.before.end as usize;
            after = hunk.after.end as usize;
        }
        while after < end && !self.truncated {
            self.source(ReviewLineKind::Context, current, Some(before), Some(after));
            before += 1;
            after += 1;
        }
    }
}

fn line_count(rope: &Rope) -> usize {
    rope.len_lines(LINE_TYPE)
        - usize::from(rope.len() == 0 || get_line_ending(&rope.slice(..)).is_some())
}

fn range_label(range: &Range<usize>) -> String {
    format!(
        "{},{}",
        if range.is_empty() {
            range.start
        } else {
            range.start + 1
        },
        range.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_preserves_carriage_returns_outside_crlf_endings() {
        let review = Review::new(Rope::from(""), Rope::from("first\r\r\nlast\r"), None);
        let added: Vec<_> = review
            .lines
            .iter()
            .filter(|line| line.kind == ReviewLineKind::Added)
            .map(|line| line.text.as_str())
            .collect();
        if LINE_TYPE == editor_core::ropey::LineType::LF {
            assert_eq!(added, ["first\r", "last\r"]);
        } else {
            assert_eq!(added, ["first", "", "last"]);
        }
    }

    #[test]
    fn syntax_ranges_do_not_create_empty_emphasis_on_blank_rows() {
        let before = Rope::from("");
        let after = Rope::from("a\n\nb\n");
        let mut review = Review::new(before.clone(), after.clone(), None);
        review.annotate(
            &before,
            &after,
            Changes {
                before: vec![],
                after: std::iter::once(0..5).collect(),
            },
        );
        assert!(review
            .lines
            .iter()
            .flat_map(|line| line.changes.iter().flatten())
            .all(|range| !range.is_empty()));
    }

    #[test]
    fn review_navigation_maps_deletions_without_crossing_hunk_boundaries() {
        for (before, after, boundary) in [
            ("removed\nkeep\n", "keep\n", 0),
            ("keep\nremoved\nend\n", "keep\nend\n", 1),
            ("keep\nremoved\n", "keep\n", 1),
            ("removed\n", "", 0),
        ] {
            let review = Review::new(Rope::from(before), Rope::from(after), None);
            let row = review
                .lines
                .iter()
                .position(|line| line.kind == ReviewLineKind::Removed)
                .unwrap();
            assert_eq!(review.after_line_at(row), Some(boundary));
            assert_eq!(review.after_line_at(usize::MAX), None);
        }
        let before: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let after = before.replace("line 2\n", "").replace("line 25\n", "new\n");
        let review = Review::new(Rope::from(before), Rope::from(after), None);
        let row = review
            .lines
            .iter()
            .position(|line| line.kind == ReviewLineKind::Removed)
            .unwrap();
        assert_eq!(review.after_line_at(row), Some(2));
        assert_eq!(Review::notice("No text changes").after_line_at(0), None);
    }

    #[test]
    fn syntax_spans_are_clipped_to_literal_rows_and_keep_exact_hunks() {
        let before = Rope::from("λ old\r\nend\n");
        let after = Rope::from("λ new\r\nend\n");
        let mut review = Review::new(before.clone(), after.clone(), Some(0));
        review.annotate(
            &before,
            &after,
            Changes {
                before: std::iter::once(3..6).collect(),
                after: std::iter::once(3..8).collect(),
            },
        );
        assert_eq!((review.additions, review.deletions), (1, 1));
        assert_eq!(
            review.selected_hunk(),
            Some(&Hunk {
                before: 0..1,
                after: 0..1,
            })
        );
        let line = review
            .lines
            .iter()
            .find(|line| line.kind == ReviewLineKind::Added)
            .unwrap();
        assert_eq!(line.text, "λ new");
        assert_eq!(line.changes.as_deref(), Some(std::slice::from_ref(&(3..6))));
    }

    #[test]
    fn review_retains_deleted_text_and_both_source_locations() {
        let review = Review::new(
            Rope::from("one\nold\nthree\n"),
            Rope::from("one\nnew\nthree\n"),
            None,
        );
        let lines: Vec<_> = review
            .lines
            .iter()
            .map(|line| (line.kind, line.before, line.after, line.text.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                (ReviewLineKind::Header, None, None, "@@ -1,3 +1,3 @@"),
                (ReviewLineKind::Context, Some(0), Some(0), "one"),
                (ReviewLineKind::Removed, Some(1), None, "old"),
                (ReviewLineKind::Added, None, Some(1), "new"),
                (ReviewLineKind::Context, Some(2), Some(2), "three"),
            ]
        );
        assert_eq!((review.additions, review.deletions), (1, 1));
    }

    #[test]
    fn review_handles_deletion_empty_files_crlf_unicode_and_missing_newlines() {
        let review = Review::new(Rope::from("žluťoučký\r\nold"), Rope::from(""), Some(0));
        assert_eq!(review.deletions, 2);
        assert!(review.lines.iter().any(|line| line.text == "žluťoučký"));
        assert!(review
            .lines
            .iter()
            .any(|line| line.text == "\\ No newline at end of file"));
        let review = Review::new(Rope::from(""), Rope::from("new\n"), None);
        assert_eq!((review.additions, review.deletions), (1, 0));
        assert!(!review
            .lines
            .iter()
            .any(|line| line.kind == ReviewLineKind::Added && line.text.is_empty()));
    }

    #[test]
    fn eof_deletions_remain_inspectable_and_large_models_are_bounded() {
        let review = Review::new(Rope::from("one\ntwo\n"), Rope::from("one\n"), Some(1));
        assert_eq!((review.additions, review.deletions), (0, 1));
        assert_eq!(
            review.selected_hunk(),
            Some(&Hunk {
                before: 1..2,
                after: 1..1,
            })
        );
        assert!(review
            .lines
            .iter()
            .any(|line| line.kind == ReviewLineKind::Removed && line.text == "two"));
        let review = Review::new(
            Rope::from(""),
            Rope::from("line\n".repeat(MAX_ROWS + 1)),
            Some(0),
        );
        assert!(review.truncated);
        assert!(review.selected_hunk().is_none());
        assert!(review.lines.len() <= MAX_ROWS + 1);
        assert!(review.lines.last().unwrap().text.contains("truncated"));
        let review = Review::new(Rope::from("\n".repeat(200_001)), Rope::from(""), None);
        assert!(review.truncated);
        assert!(review.selected_hunk().is_none());
        assert_eq!(review.lines.len(), 1);
        assert!(review.lines[0].text.contains("too large"));
    }

    #[test]
    fn hunk_context_after_large_deletions_retains_correct_source_numbers() {
        let base: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let changed: String = (10..20)
            .map(|i| {
                if i == 11 {
                    "changed\n".into()
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        let before = Rope::from(base.as_str());
        let after = Rope::from(changed.as_str());
        let review = Review::new(before.clone(), after.clone(), Some(1));
        assert!(review
            .lines
            .iter()
            .any(|line| line.kind == ReviewLineKind::Added && line.text == "changed"));
        for line in review.lines {
            if let Some(index) = line.before {
                assert_eq!(
                    line.text,
                    before.line(index, LINE_TYPE).to_string().trim_end()
                );
            }
            if let Some(index) = line.after {
                assert_eq!(
                    line.text,
                    after.line(index, LINE_TYPE).to_string().trim_end()
                );
            }
        }
    }

    #[test]
    fn nearby_hunks_share_context_and_distant_hunks_can_be_inspected_alone() {
        let base: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let changed = base
            .replace("line 2\n", "changed two\n")
            .replace("line 5\n", "changed five\n")
            .replace("line 25\n", "changed twenty-five\n");
        let review = Review::new(
            Rope::from(base.as_str()),
            Rope::from(changed.as_str()),
            None,
        );
        assert!(review.selected_hunk().is_none());
        assert_eq!(
            review
                .lines
                .iter()
                .filter(|line| line.kind == ReviewLineKind::Header)
                .count(),
            2
        );
        let review = Review::new(
            Rope::from(base.as_str()),
            Rope::from(changed.as_str()),
            Some(2),
        );
        assert_eq!((review.additions, review.deletions), (2, 2));
        assert_eq!(
            review.selected_hunk(),
            Some(&Hunk {
                before: 2..3,
                after: 2..3,
            })
        );
        let review = Review::new(
            Rope::from(base.as_str()),
            Rope::from(changed.as_str()),
            Some(25),
        );
        assert_eq!((review.additions, review.deletions), (1, 1));
        assert!(!review.lines.iter().any(|line| line.text == "changed two"));
        assert_eq!(
            review.selected_hunk(),
            Some(&Hunk {
                before: 25..26,
                after: 25..26,
            })
        );
        let review = Review::new(Rope::from(base), Rope::from(changed), Some(3));
        assert!(review.selected_hunk().is_none());
        assert_eq!(review.lines[0].text, "No text changes");
    }
}
