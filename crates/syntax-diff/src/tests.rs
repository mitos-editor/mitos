use super::*;

fn provider(name: &str) -> SyntaxDiff {
    SyntaxDiff::new(
        loader::syntax::Resources::default()
            .grammar(name)
            .unwrap()
            .unwrap(),
    )
}

#[test]
fn comparison_resets_a_cached_parsers_included_ranges() {
    use tree_house::tree_sitter::{Point, Range as SyntaxRange};
    let provider = provider("json");
    {
        let mut parser = Parser::new();
        parser.set_grammar(provider.grammar).unwrap();
        parser
            .set_included_ranges(&[SyntaxRange::new(
                Point::ZERO,
                Point { row: 0, col: 1 },
                0,
                1,
            )])
            .unwrap();
    }
    let before = Rope::from(" \n{\"value\":1}\n");
    let after = Rope::from(" \n{\"value\":2}\n");
    let changes = provider.compare(&before, &after, &|| false).unwrap();
    assert!(!changes.before.is_empty());
    assert!(!changes.after.is_empty());
}

#[test]
fn reflowed_code_matches_across_lines_and_literal_edits_keep_precise_sources() {
    let before = Rope::from("fn main() { call(1, 2); }\n");
    let after = Rope::from("fn main() {\n    call(\n        1,\n        3\n    );\n}\n");
    let changes = provider("rust")
        .compare(&before, &after, &|| false)
        .unwrap();
    let fragments = |text: &Rope, ranges: Vec<Range<usize>>| {
        ranges
            .into_iter()
            .map(|range| text.slice(range).to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(fragments(&before, changes.before), ["2"]);
    assert_eq!(fragments(&after, changes.after), ["3"]);
}

#[test]
fn formatting_only_changes_have_no_syntax_changes_but_keep_exact_textual_hunks() {
    let before = Rope::from("{\"a\": [1, 2]}\n");
    let after = Rope::from("{\r\n  \"a\": [\r\n    1,\r\n    2\r\n  ]\r\n}\r\n");
    let review =
        text_diff::Review::with_provider(before, after, None, &provider("json"), &|| false);
    assert_eq!(review.provider, Some("syntax"));
    assert!(review.additions > 0 && review.deletions > 0);
    assert!(review
        .lines
        .iter()
        .filter_map(|line| line.changes.as_ref())
        .all(Vec::is_empty));
}

#[test]
fn unicode_strings_and_comments_keep_literal_changes() {
    for (language, before, after, changed) in [
        ("json", "{\"name\":\" \"}\n", "{\"name\":\"  \"}\n", "  "),
        (
            "json",
            "{\"name\":\"λ🚀\"}\n",
            "{\"name\":\"λ🌍\"}\n",
            "λ🌍",
        ),
        (
            "rust",
            "fn main() { /* first */ }\n",
            "fn main() { /* second */ }\n",
            "second",
        ),
    ] {
        let after = Rope::from(after);
        let changes = provider(language)
            .compare(&Rope::from(before), &after, &|| false)
            .unwrap();
        assert!(
            changes
                .after
                .iter()
                .any(|range| after.slice(range.clone()).to_string().contains(changed)),
            "{language}: {changes:?}"
        );
    }
}

#[test]
fn grammar_structure_distinguishes_significant_python_indentation() {
    let before = Rope::from("if ready:\n    first()\n    second()\n");
    let after = Rope::from("if ready:\n    first()\nsecond()\n");
    let changes = provider("python")
        .compare(&before, &after, &|| false)
        .unwrap();
    assert!(!changes.before.is_empty() || !changes.after.is_empty());
}

#[test]
fn additions_deletions_and_repeated_delimiters_have_valid_ordered_spans() {
    for (before, after) in [
        ("", "[1, 1, [1]]\n"),
        ("[1, 1, [1]]\n", ""),
        ("[[],[[]],[]]", "[[[]],[],[[]]]"),
    ] {
        let before = Rope::from(before);
        let after = Rope::from(after);
        let review =
            text_diff::Review::with_provider(before, after, None, &provider("json"), &|| false);
        assert_eq!(review.provider, Some("syntax"), "{:?}", review.fallback);
        for line in review.lines {
            for range in line.changes.into_iter().flatten() {
                assert!(line.text.is_char_boundary(range.start));
                assert!(line.text.is_char_boundary(range.end));
                assert!(range.start < range.end && range.end <= line.text.len());
            }
        }
    }
}

#[test]
fn unsupported_syntax_cancellation_and_large_inputs_fall_back() {
    let provider = provider("json");
    assert_eq!(
        provider
            .compare(&Rope::from("{}"), &Rope::from("{]"), &|| false)
            .unwrap_err(),
        ReviewUnavailable("syntax contains parse errors")
    );
    assert_eq!(
        provider
            .compare(&Rope::from("{}"), &Rope::from("[]"), &|| true)
            .unwrap_err(),
        ReviewUnavailable("comparison canceled")
    );
    let large = Rope::from(" ".repeat(MAX_BYTES + 1));
    assert!(provider
        .compare(&large, &Rope::from("{}"), &|| false)
        .unwrap_err()
        .0
        .contains("size limit"));
    let nodes = Rope::from(format!("[{}0]", "0,".repeat(20_000)));
    // Busy runners can exhaust the time budget before reaching the event limit.
    let reason = provider
        .compare(&nodes, &Rope::from("[]"), &|| false)
        .unwrap_err()
        .0;
    assert!(
        reason.contains("limit") || reason.contains("budget") || reason.contains("interrupted")
    );
}
