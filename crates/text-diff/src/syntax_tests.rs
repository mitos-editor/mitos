use super::*;

#[test]
fn whitespace_inside_literals_remains_significant() {
    for (language, before, after) in [
        (
            "rust",
            "fn f() { let s = r#\" \"#; }",
            "fn f() { let s = r#\"  \"#; }",
        ),
        ("python", "x = f\" \"\n", "x = f\"  \"\n"),
        ("javascript", "const x = ` `;", "const x = `  `;"),
        (
            "yaml",
            "x: |\n  first\n  \n  last\n",
            "x: |\n  first\n   \n  last\n",
        ),
        ("bash", "cat <<EOF\n \nEOF\n", "cat <<EOF\n  \nEOF\n"),
        ("html", "<pre> </pre>", "<pre>  </pre>"),
        (
            "html",
            "<span>a</span> <span>b</span>",
            "<span>a</span>  <span>b</span>",
        ),
        (
            "javascript",
            "const x = <pre> </pre>;",
            "const x = <pre>  </pre>;",
        ),
    ] {
        let changes = parsed(language)
            .compare(&Rope::from(before), &Rope::from(after), &|| false)
            .unwrap();
        assert!(
            !changes.before.is_empty() && !changes.after.is_empty(),
            "{language}: {changes:?}"
        );
    }
}

use tree_house::tree_sitter::{Grammar, Parser, RopeInput};

struct ParsedComparison {
    grammar: Grammar,
}

fn parsed(name: &str) -> ParsedComparison {
    ParsedComparison {
        grammar: loader::syntax::Resources::default()
            .grammar(name)
            .unwrap()
            .unwrap(),
    }
}

impl ParsedComparison {
    fn parse(&self, text: &Rope) -> Tree {
        let mut parser = Parser::new();
        parser.set_grammar(self.grammar).unwrap();
        parser.set_included_ranges(&[]).unwrap();
        parser.parse(RopeInput::new(text.slice(..)), None).unwrap()
    }

    fn compare(&self, before: &Rope, after: &Rope, canceled: &dyn Fn() -> bool) -> Option<Changes> {
        compare(
            before,
            after,
            &self.parse(before),
            &self.parse(after),
            canceled,
        )
    }

    fn review(&self, before: Rope, after: Rope) -> crate::Review {
        let before_tree = self.parse(&before);
        let after_tree = self.parse(&after);
        crate::Review::new(before.clone(), after.clone(), None).with_syntax(
            &before,
            &after,
            &before_tree,
            &after_tree,
            &|| false,
        )
    }
}

#[test]
fn reflowed_code_matches_across_lines_and_literal_edits_keep_precise_sources() {
    let before = Rope::from("fn main() { call(1, 2); }\n");
    let after = Rope::from("fn main() {\n    call(\n        1,\n        3\n    );\n}\n");
    let changes = parsed("rust").compare(&before, &after, &|| false).unwrap();
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
    let review = parsed("json").review(before, after);
    assert!(review.layout_only);
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
        let changes = parsed(language)
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
    let changes = parsed("python")
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
        let review = parsed("json").review(before, after);
        assert!(review
            .lines
            .iter()
            .filter(|line| matches!(
                line.kind,
                crate::ReviewLineKind::Removed | crate::ReviewLineKind::Added
            ))
            .all(|line| line.changes.is_some()));
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
fn invalid_syntax_cancellation_and_large_inputs_keep_textual_emphasis() {
    let parsed = parsed("json");
    assert!(parsed
        .compare(&Rope::from("{}"), &Rope::from("{]"), &|| false)
        .is_none());
    let review = parsed.review(Rope::from("{}"), Rope::from("{]"));
    assert!(review.lines.iter().any(|line| line.text == "{]"));
    assert!(review.lines.iter().all(|line| line.changes.is_none()));
    assert!(parsed
        .compare(&Rope::from("{}"), &Rope::from("[]"), &|| true)
        .is_none());
    let large = Rope::from(" ".repeat(MAX_BYTES + 1));
    assert!(parsed
        .compare(&large, &Rope::from("{}"), &|| false)
        .is_none());
    let nodes = Rope::from(format!("[{}0]", "0,".repeat(20_000)));
    assert!(parsed
        .compare(&nodes, &Rope::from("[]"), &|| false)
        .is_none());
}
