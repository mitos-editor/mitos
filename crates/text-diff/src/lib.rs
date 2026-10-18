//! Text comparison independent of repository services and task scheduling.

mod line_cache;
mod provider;
mod review;
mod transaction;
pub use provider::{ReviewChanges, ReviewProvider, ReviewUnavailable};
pub use review::{Review, ReviewLine, ReviewLineKind, MAX_REVIEW_BYTES, MAX_REVIEW_LINES};

use editor_core::Rope;
pub use imara_diff::Hunk;
use imara_diff::{Algorithm, IndentHeuristic, IndentLevel};
use line_cache::InternedRopeLines;
pub use transaction::compare_ropes;

const MAX_DIFF_LINES: usize = 64 * u16::MAX as usize;
const MAX_DIFF_BYTES: usize = MAX_DIFF_LINES * 128;

// Tree-backed Ropey 2 slices allocate chunk cursors even for a single leaf.
// Keep short contiguous line tokens as borrowed strings to avoid those allocations
// during hashing, equality, and character iteration. Long lines retain rope metrics.
fn line_token(line: editor_core::RopeSlice<'_>) -> editor_core::RopeSlice<'_> {
    line.as_str().map_or(line, editor_core::RopeSlice::from)
}

/// Reusable line interning and diff allocations for a base and its edited text.
/// Callers own scheduling, publication, and synchronization.
pub struct LineDiff {
    lines: InternedRopeLines,
    diff: imara_diff::Diff,
}

impl LineDiff {
    pub fn new(base: Rope, document: Rope) -> Self {
        let mut diff = Self {
            lines: InternedRopeLines::new(base, document),
            diff: imara_diff::Diff::default(),
        };
        diff.compute();
        diff
    }

    /// Update either snapshot, retaining base-line interning when only the document changes.
    pub fn update(&mut self, base: Option<Rope>, document: Option<Rope>) {
        if let Some(base) = base {
            self.lines.update_diff_base(base, document);
        } else if let Some(document) = document {
            self.lines.update_doc(document);
        } else {
            return;
        }
        self.compute();
    }

    pub fn base(&self) -> Rope {
        self.lines.diff_base()
    }

    pub fn document(&self) -> Rope {
        self.lines.doc()
    }

    pub fn hunks(&self) -> impl Iterator<Item = Hunk> + '_ {
        self.diff.hunks()
    }

    fn compute(&mut self) {
        let Some(input) = self.lines.interned_lines() else {
            return;
        };
        self.diff.compute_with(
            Algorithm::Histogram,
            &input.before,
            &input.after,
            input.interner.num_tokens(),
        );
        self.diff.postprocess_with(
            &input.before,
            &input.after,
            IndentHeuristic::new(|token| {
                IndentLevel::for_ascii_line(input.interner[token].bytes(), 4)
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_tokens_keep_multichunk_lines_and_survive_base_replacement() {
        let long = "α😀".repeat(2000);
        let base = Rope::from_str(&format!("prefix α\n{long}\nend 😀\n"));
        let changed = Rope::from_str(&format!("changed α\n{long}\nend 😀\n"));
        assert!(base.line(1, stdx::rope::LINE_TYPE).as_str().is_none());
        let mut diff = LineDiff::new(base.clone(), changed.clone());
        let hunk = diff.hunks().next().unwrap();
        assert_eq!(hunk.before, 0..1);
        assert_eq!(hunk.after, 0..1);
        diff.update(Some(changed.clone()), None);
        assert_eq!(diff.hunks().count(), 0);
        diff.update(None, Some(base));
        let hunk = diff.hunks().next().unwrap();
        assert_eq!(hunk.before, 0..1);
        assert_eq!(hunk.after, 0..1);
    }

    #[test]
    fn line_diffs_reuse_snapshots_across_document_and_base_updates() {
        let mut diff = LineDiff::new(Rope::from("one\ntwo\n"), Rope::from("one\nchanged\n"));
        let hunk = diff.hunks().next().unwrap();
        assert_eq!(hunk.before, 1..2);
        assert_eq!(hunk.after, 1..2);
        diff.update(None, Some(Rope::from("one\ntwo\n")));
        assert_eq!(diff.hunks().count(), 0);
        diff.update(
            Some(Rope::from("different\n")),
            Some(Rope::from("different\n")),
        );
        assert_eq!(diff.hunks().count(), 0);
        diff.update(None, Some(Rope::from("extra\ndifferent\n")));
        let hunk = diff.hunks().next().unwrap();
        assert_eq!(hunk.before, 0..0);
        assert_eq!(hunk.after, 0..1);
    }
}
