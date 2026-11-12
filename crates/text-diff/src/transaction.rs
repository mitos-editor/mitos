use std::ops::Range;
use std::time::Instant;

use editor_core::{ropey, RopeSlice};
use gix_imara_diff::{Algorithm, Diff, Hunk, InternedInput};

use editor_core::{ChangeSet, ChangeSetBuilder as ChangesBuilder, Rope, Tendril, Transaction};

// Line counts alone do not bound refinement for minified or generated text.
const MAX_CHAR_DIFF_BYTES: usize = 64 * 1024;

struct ChangeSetBuilder<'a> {
    res: ChangesBuilder,
    after: RopeSlice<'a>,
    file: &'a InternedInput<RopeSlice<'a>>,
    current_hunk: InternedInput<char>,
    char_diff: Diff,
    pos: u32,
}

impl ChangeSetBuilder<'_> {
    fn process_hunk(&mut self, before: Range<u32>, after: Range<u32>) {
        let len = self.file.before[self.pos as usize..before.start as usize]
            .iter()
            .map(|&it| self.file.interner[it].len_chars())
            .sum();
        self.res.retain(len);
        self.pos = before.end;

        let len_before = before.end - before.start;
        let len_after = after.end - after.start;
        let before_lines = &self.file.before[before.start as usize..before.end as usize];
        let after_lines = &self.file.after[after.start as usize..after.end as usize];

        // Pure insertions/removals do not require a character diff.
        // Replace large or disproportionate hunks directly to bound character refinement.
        if len_before == 0
            || len_after == 0
            || len_after > 5 * len_before
            || 5 * len_after < len_before && len_before > 10
            || len_before + len_after > 200
            || before_lines
                .iter()
                .chain(after_lines)
                .map(|&token| self.file.interner[token].len())
                .sum::<usize>()
                > MAX_CHAR_DIFF_BYTES
        {
            let remove = before_lines
                .iter()
                .map(|&it| self.file.interner[it].len_chars())
                .sum();
            self.res.delete(remove);
            let mut fragment = Tendril::new();
            if len_after > 500 {
                // copying a rope line by line is slower then copying the entire
                // rope. Use to_string for very large changes instead..
                let byte_offset = |line| {
                    if line == self.file.after.len() {
                        self.after.len()
                    } else {
                        self.after.line_to_byte_idx(line, stdx::rope::LINE_TYPE)
                    }
                };
                fragment = self
                    .after
                    .slice(byte_offset(after.start as usize)..byte_offset(after.end as usize))
                    .to_string()
                    .into();
            } else {
                for &line in after_lines {
                    for chunk in self.file.interner[line].chunks() {
                        fragment.push_str(chunk)
                    }
                }
            };
            self.res.insert(fragment);
        } else {
            // Character granularity preserves positions within surviving text.
            let hunk_before = before_lines
                .iter()
                .flat_map(|&it| self.file.interner[it].chars());
            let hunk_after = after_lines
                .iter()
                .flat_map(|&it| self.file.interner[it].chars());
            self.current_hunk.update_before(hunk_before);
            self.current_hunk.update_after(hunk_after);
            // the histogram heuristic does not work as well
            // for characters because the same characters often reoccur
            // use myer diff instead
            self.char_diff.compute_with(
                Algorithm::Myers,
                &self.current_hunk.before,
                &self.current_hunk.after,
                self.current_hunk.interner.num_tokens(),
            );
            append_character_changes(&mut self.res, &self.current_hunk, &self.char_diff);
            // reuse allocations
            self.current_hunk.clear();
        }
    }

    fn finish(mut self) -> ChangeSet {
        let len = self.file.before[self.pos as usize..]
            .iter()
            .map(|&it| self.file.interner[it].len_chars())
            .sum();

        self.res.retain(len);
        self.res.finish()
    }
}

// Shared by bounded document diffs and precise edits of selected fragments.
fn append_character_changes(
    changes: &mut ChangesBuilder,
    input: &InternedInput<char>,
    diff: &Diff,
) {
    let mut pos = 0;
    for Hunk { before, after } in diff.hunks() {
        changes.retain((before.start - pos) as usize);
        changes.delete(before.len());
        pos = before.end;
        changes.insert(
            input.after[after.start as usize..after.end as usize]
                .iter()
                .map(|&token| input.interner[token])
                .collect(),
        );
    }
    changes.retain(input.before.len() - pos as usize);
}

/// Compare a selected fragment at character granularity, retaining unchanged anchors.
/// Unlike document diffs, reflow must not fall back to replacing an entire paragraph.
pub fn compare_fragments(before: RopeSlice<'_>, after: &str) -> Transaction {
    let mut input = InternedInput::default();
    input.update_before(before.chars());
    input.update_after(after.chars());
    let diff = Diff::compute(Algorithm::Myers, &input);
    let mut changes = ChangesBuilder::with_capacity(32);
    append_character_changes(&mut changes, &input, &diff);
    changes.finish().into()
}

struct RopeLines<'a>(RopeSlice<'a>);

impl<'a> gix_imara_diff::TokenSource for RopeLines<'a> {
    type Token = RopeSlice<'a>;
    type Tokenizer = std::iter::Map<ropey::iter::Lines<'a>, fn(RopeSlice<'a>) -> RopeSlice<'a>>;

    fn tokenize(&self) -> Self::Tokenizer {
        self.0.lines(stdx::rope::LINE_TYPE).map(super::line_token)
    }

    fn estimate_tokens(&self) -> u32 {
        // we can provide a perfect estimate which is very nice for performance
        self.0.len_lines(stdx::rope::LINE_TYPE) as u32
    }
}

/// Compares `old` and `new` to generate a [`Transaction`] describing
/// the steps required to get from `old` to `new`.
pub fn compare_ropes(before: &Rope, after: &Rope) -> Transaction {
    let start = Instant::now();
    let res = ChangesBuilder::with_capacity(32);
    let after = after.slice(..);
    let file = InternedInput::new(RopeLines(before.slice(..)), RopeLines(after));
    let mut builder = ChangeSetBuilder {
        res,
        file: &file,
        after,
        pos: 0,
        current_hunk: InternedInput::default(),
        char_diff: Diff::default(),
    };
    let mut diff = Diff::default();
    super::diff_lines(&mut diff, &file);
    for hunk in diff.hunks() {
        builder.process_hunk(hunk.before, hunk.after)
    }
    let res = builder.finish().into();

    log::debug!(
        "rope diff took {}s",
        Instant::now().duration_since(start).as_secs_f64()
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity(a: &str, b: &str) {
        let mut old = Rope::from(a);
        let new = Rope::from(b);
        compare_ropes(&old, &new).apply(&mut old);
        assert_eq!(old, new);
    }

    #[test]
    fn large_single_line_edits_skip_character_refinement() {
        let before = Rope::from(format!(
            "prefix {} suffix\n",
            "α".repeat(MAX_CHAR_DIFF_BYTES)
        ));
        let after = Rope::from(format!(
            "prefix {} suffix\n",
            "β".repeat(MAX_CHAR_DIFF_BYTES)
        ));
        let change = compare_ropes(&before, &after);
        assert_eq!(change.changes().changes().len(), 2);
        let mut actual = before.clone();
        assert!(change.apply(&mut actual));
        assert_eq!(actual, after);
        assert!(change.invert(&before).apply(&mut actual));
        assert_eq!(actual, before);
    }

    quickcheck::quickcheck! {
        fn test_compare_ropes(a: String, b: String) -> bool {
            let mut old = Rope::from(a);
            let new = Rope::from(b);
            compare_ropes(&old, &new).apply(&mut old);
            old == new
        }

        fn test_compare_fragments(a: String, b: String) -> bool {
            let before = Rope::from(a);
            let mut actual = before.clone();
            let change = compare_fragments(before.slice(..), &b);
            change.apply(&mut actual) && actual == b.as_str()
                && change.invert(&before).apply(&mut actual) && actual == before
        }

    }

    #[test]
    fn large_unicode_insertions_preserve_prefixes_and_suffixes() {
        let lines: Vec<_> = (0..1200).map(|i| format!("{i} α😀 café\r\n")).collect();
        let original = lines.concat();
        let inserted = "inserted 界👩🏽‍💻\r\n".repeat(1024);
        test_identity("", &inserted);
        for line in [0, 600, 1200] {
            let prefix = lines[..line].concat();
            let suffix = lines[line..].concat();
            test_identity(&original, &format!("{prefix}{inserted}{suffix}"));
        }
    }

    #[test]
    fn reflow_preserves_positions_when_a_crlf_paragraph_expands() {
        let words: Vec<_> = (0..32).map(|i| format!("w{i:02}")).collect();
        let before = Rope::from(format!("{}\r\n", words.join(" ")));
        let selection = editor_core::Selection::single(0, before.len_chars());
        let reflow = editor_core::transform::reflow_selections(
            &before,
            &selection,
            7,
            editor_core::LineEnding::Crlf,
        );
        let mut after = before.clone();
        assert!(reflow.apply(&mut after));
        let change = compare_fragments(before.slice(..), &after.to_string());
        let saved = editor_core::Selection::point(32).map(change.changes());
        assert_eq!(saved.primary().head, 36);
        let mut actual = before.clone();
        assert!(change.apply(&mut actual));
        assert_eq!(actual, after);
        assert!(change.invert(&before).apply(&mut actual));
        assert_eq!(actual, before);
    }

    #[test]
    fn fragment_edits_preserve_unicode_and_reflowed_comment_prefixes() {
        let before = Rope::from("// α😀 one\n// 界 two\n");
        let after = "// α😀 one 界 two\r\n";
        let change = compare_fragments(before.slice(..), after);
        let saved = editor_core::Selection::point(13).map(change.changes());
        assert_eq!(saved.primary().head, 10);
        let mut actual = before.clone();
        assert!(change.apply(&mut actual));
        assert_eq!(actual, after);
    }

    #[test]
    fn equal_files() {
        test_identity("foo", "foo");
    }

    #[test]
    fn trailing_newline() {
        test_identity("foo\n", "foo");
        test_identity("foo", "foo\n");
    }

    #[test]
    fn new_file() {
        test_identity("", "foo");
    }

    #[test]
    fn deleted_file() {
        test_identity("foo", "");
    }
}
