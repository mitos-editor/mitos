//! Unified, source-mapped review rows independent of VCS and frontend rendering.

use std::ops::Range;

use editor_core::Rope;
use stdx::rope::{RopeExt as _, LINE_TYPE};

use crate::{Hunk, LineDiff};

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

#[derive(Debug)]
pub struct ReviewLine {
    pub kind: ReviewLineKind,
    /// Zero-based source lines. Headers and notices have neither.
    pub before: Option<usize>,
    pub after: Option<usize>,
    pub text: String,
}

/// An immutable review snapshot. Text is literal and never parsed as Markdown.
#[derive(Debug, Default)]
pub struct Review {
    pub lines: Vec<ReviewLine>,
    pub additions: usize,
    pub deletions: usize,
    pub truncated: bool,
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
            });
        }
        review
    }

    pub fn max_line_chars(&self) -> usize {
        self.max_line_chars
    }

    pub fn line_number_width(&self) -> usize {
        self.largest_line.to_string().len().max(1)
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
        let text = text.to_string();
        let content = text.trim_end_matches(['\r', '\n']).to_owned();
        self.push(kind, before, after, content);
        if kind != ReviewLineKind::Context && !text.ends_with('\n') {
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
        - usize::from(rope.len_chars() == 0 || rope.char_at(rope.len_chars() - 1) == '\n')
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
        assert!(review
            .lines
            .iter()
            .any(|line| line.kind == ReviewLineKind::Removed && line.text == "two"));
        let review = Review::new(
            Rope::from(""),
            Rope::from("line\n".repeat(MAX_ROWS + 1)),
            None,
        );
        assert!(review.truncated);
        assert!(review.lines.len() <= MAX_ROWS + 1);
        assert!(review.lines.last().unwrap().text.contains("truncated"));
        let review = Review::new(Rope::from("\n".repeat(200_001)), Rope::from(""), None);
        assert!(review.truncated);
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
            Some(25),
        );
        assert_eq!((review.additions, review.deletions), (1, 1));
        assert!(!review.lines.iter().any(|line| line.text == "changed two"));
    }
}
