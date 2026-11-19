use std::time::Instant;

use editor_core::{ropey, ChangeSetBuilder, Rope, RopeSlice, Transaction};
use gix_imara_diff::{Algorithm, Diff, Hunk, InternedInput};
use stdx::rope::LINE_TYPE;

// Line counts alone do not bound refinement for minified or generated text.
const MAX_CHAR_DIFF_BYTES: usize = 64 * 1024;

// Shared by bounded document diffs and precise edits of selected fragments.
fn append_character_changes(
    changes: &mut ChangeSetBuilder,
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
    let mut changes = ChangeSetBuilder::with_capacity(32);
    append_character_changes(&mut changes, &input, &diff);
    changes.finish().into()
}

struct RopeLines<'a>(RopeSlice<'a>);

impl<'a> gix_imara_diff::TokenSource for RopeLines<'a> {
    type Token = RopeSlice<'a>;
    type Tokenizer = std::iter::Map<ropey::iter::Lines<'a>, fn(RopeSlice<'a>) -> RopeSlice<'a>>;

    fn tokenize(&self) -> Self::Tokenizer {
        self.0.lines(LINE_TYPE).map(super::line_token)
    }

    fn estimate_tokens(&self) -> u32 {
        // we can provide a perfect estimate which is very nice for performance
        self.0.len_lines(LINE_TYPE) as u32
    }
}

/// Build an undoable transaction that transforms `before` into `after`.
pub fn compare_ropes(before: &Rope, after: &Rope) -> Transaction {
    let start = Instant::now();
    let before = before.slice(..);
    let after = after.slice(..);
    let file = InternedInput::new(RopeLines(before), RopeLines(after));
    let mut diff = Diff::default();
    super::diff_lines(&mut diff, &file);
    let mut changes = ChangeSetBuilder::with_capacity(32);
    let mut current_hunk = InternedInput::default();
    let mut char_diff = Diff::default();
    let mut pos = 0;
    for hunk in diff.hunks() {
        let before_start = before.line_to_byte_idx(hunk.before.start as usize, LINE_TYPE);
        let before_text = before
            .slice(before_start..before.line_to_byte_idx(hunk.before.end as usize, LINE_TYPE));
        let after_text = after.slice(
            after.line_to_byte_idx(hunk.after.start as usize, LINE_TYPE)
                ..after.line_to_byte_idx(hunk.after.end as usize, LINE_TYPE),
        );
        let char_start = before.byte_to_char_idx(before_start);
        changes.retain(char_start - pos);
        pos = char_start + before_text.len_chars();

        let len_before = hunk.before.len();
        let len_after = hunk.after.len();
        // Pure insertions/removals do not require a character diff.
        // Replace large or disproportionate hunks directly to bound character refinement.
        if len_before == 0
            || len_after == 0
            || len_after > 5 * len_before
            || 5 * len_after < len_before && len_before > 10
            || len_before + len_after > 200
            || before_text.len() + after_text.len() > MAX_CHAR_DIFF_BYTES
        {
            changes.delete(before_text.len_chars());
            changes.insert(after_text.chunks().collect());
        } else {
            // Character granularity preserves positions within surviving text.
            current_hunk.update_before(before_text.chars());
            current_hunk.update_after(after_text.chars());
            // Small, repeated character tokens favor Myers over Histogram.
            char_diff.compute_with(
                Algorithm::Myers,
                &current_hunk.before,
                &current_hunk.after,
                current_hunk.interner.num_tokens(),
            );
            append_character_changes(&mut changes, &current_hunk, &char_diff);
            // Reuse allocations between hunks.
            current_hunk.clear();
        }
    }
    changes.retain(before.len_chars() - pos);
    let transaction = changes.finish().into();

    log::debug!("rope diff took {}s", start.elapsed().as_secs_f64());
    transaction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity(a: &str, b: &str) {
        let before = Rope::from(a);
        let mut old = before.clone();
        let new = Rope::from(b);
        let change = compare_ropes(&before, &new);
        assert!(change.apply(&mut old));
        assert_eq!(old, new);
        assert!(change.invert(&before).apply(&mut old));
        assert_eq!(old, before);
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
            let before = Rope::from(a);
            let mut actual = before.clone();
            let new = Rope::from(b);
            let change = compare_ropes(&before, &new);
            change.apply(&mut actual) && actual == new
                && change.invert(&before).apply(&mut actual) && actual == before
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
        test_identity(&inserted, "");
        for line in [0, 600, 1200] {
            let prefix = lines[..line].concat();
            let suffix = lines[line..].concat();
            let changed = format!("{prefix}{inserted}{suffix}");
            test_identity(&original, &changed);
            test_identity(&changed, &original);
        }
    }

    #[test]
    fn document_edits_preserve_unicode_positions_across_separate_hunks() {
        let before = Rope::from("prefix α😀 old suffix\r\nkeep 界\r\nlast");
        let after = Rope::from("prefix α😀 replacement suffix\r\nkeep 界\r\nchanged");
        let change = compare_ropes(&before, &after);
        let suffix = "prefix α😀 old ".chars().count();
        for (from, to) in [(8, 8), (suffix, suffix + 8)] {
            assert_eq!(
                editor_core::Selection::point(from)
                    .map(change.changes())
                    .primary()
                    .head,
                to
            );
        }
        let mut actual = before.clone();
        assert!(change.apply(&mut actual));
        assert_eq!(actual, after);
        assert!(change.invert(&before).apply(&mut actual));
        assert_eq!(actual, before);
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
