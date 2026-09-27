use std::sync::LazyLock;

use editor_core::{syntax::Loader, Rope, Syntax};

static LOADER: LazyLock<Loader> =
    LazyLock::new(
        || editor_core::config::default_lang_loader(loader::syntax::Resources::default()),
    );

/// Check complete spans, not just their first byte: exclusions must not leak
/// part of a code token into the spell checker.
fn assert_scope(language: &str, text: &str, checked: &[&str], skipped: &[&str]) {
    let source = Rope::from_str(text);
    let language = LOADER.language_for_name(language).unwrap();
    let syntax = Syntax::new(source.slice(..), language, &LOADER).unwrap();
    let regions = syntax.spell_regions(source.slice(..), &LOADER, 0..source.len_bytes());
    let prose: Vec<_> = regions.iter().map(|range| &text[range.clone()]).collect();

    for needle in checked {
        assert!(text.contains(needle), "missing fixture text {needle:?}");
        for (start, _) in text.match_indices(needle) {
            let end = start + needle.len();
            assert!(
                regions.iter().any(|r| r.start <= start && end <= r.end),
                "{needle:?} at {start} should be checked; prose: {prose:?}"
            );
        }
    }
    for needle in skipped {
        assert!(text.contains(needle), "missing fixture text {needle:?}");
        for (start, _) in text.match_indices(needle) {
            let end = start + needle.len();
            assert!(
                !regions.iter().any(|r| r.start < end && start < r.end),
                "{needle:?} at {start} should be skipped; prose: {prose:?}"
            );
        }
    }
}

#[test]
fn python_checks_docstrings_after_comments_and_preserves_comment_coverage() {
    assert_scope(
        "python",
        r#"# Module comment
"""Module prose with café and an escape \N{LATIN SMALL LETTER A}."""
setting = "ordinarystring"
"""laterstring"""

class Example:
    # Class comment
    r"""Class prose."""

    @decorator
    async def method(self):
        # Method comment
        u"""Method prose."""
        return "returnvalue"

def simple():
    "Short prose."
"#,
        &[
            "comment",
            "Module",
            "Module prose with café",
            "Class prose",
            "Method prose",
            "Method",
            "Short prose",
        ],
        &[
            "LATIN SMALL LETTER A",
            "ordinarystring",
            "laterstring",
            "returnvalue",
            "decorator",
            "Example",
        ],
    );
}

#[test]
fn python_does_not_treat_other_string_expressions_as_docstrings() {
    assert_scope(
        "python",
        r#"def formatted():
    f"formattedstring {expression}"

def binary():
    b"binarystring"

def later():
    pass
    "latestring"

def tuple_expression():
    "tuplestring", other

if condition:
    "conditionalstring"
"#,
        &[],
        &[
            "formattedstring",
            "expression",
            "binarystring",
            "latestring",
            "tuplestring",
            "conditionalstring",
        ],
    );
}

#[test]
fn html_checks_visible_text_but_not_code_or_metadata() {
    assert_scope(
        "html",
        r#"<h1>Heading prose</h1>
<p class="classname">Visible café &amp; <a href="linktarget">link label</a>.</p>
<CODE>inlinecode</CODE>
<pre><span>nestedcode</span></pre>
<kbd>keybinding</kbd><samp>sampleoutput</samp>
<script>const scriptvalue = "scriptstring"; // scriptcomment
</script>
<style>/* stylecomment */ .selector { color: red; }</style>
<!-- Comment prose -->
"#,
        &[
            "Heading prose",
            "Visible café",
            "link label",
            "Comment",
            "prose",
        ],
        &[
            "classname",
            "amp",
            "linktarget",
            "inlinecode",
            "nestedcode",
            "keybinding",
            "sampleoutput",
            "scriptvalue",
            "scriptstring",
            "scriptcomment",
            "stylecomment",
            "selector",
        ],
    );
}

#[test]
fn jsx_and_tsx_share_prose_rules_and_keep_nested_rendered_text() {
    let source = r#"const element = <Widget title="propertyvalue">
  Visible prose &amp; {expression}
  {condition ? <span>Nested prose</span> : "fallbackstring"}
  <code>inlinecode</code><pre><span>nestedcode</span></pre>
</Widget>;
"#;
    for language in ["jsx", "tsx"] {
        assert_scope(
            language,
            source,
            &["Visible prose", "Nested prose"],
            &[
                "Widget",
                "propertyvalue",
                "amp",
                "expression",
                "condition",
                "fallbackstring",
                "inlinecode",
                "nestedcode",
            ],
        );
    }
}

#[test]
fn git_commit_checks_authored_prose_without_prefixes_trailers_or_diff() {
    assert_scope(
        "git-commit",
        "fixup! feat(scopeword): Subject prose\n\nBody prose.\n\nBREAKING CHANGE: Migration prose\n\nSigned-off-by: Authorname <user@example.com>\n# templatecomment\n# ------------------------ >8 ------------------------\ndiff --git a/filename b/filename\n--- a/filename\n+++ b/filename\n@@ -1 +1 @@\n-oldvalue\n+newvalue\n",
        &["Subject prose", "Body prose", "Migration prose"],
        &[
            "fixup",
            "feat",
            "scopeword",
            "BREAKING CHANGE",
            "Signed-off-by",
            "Authorname",
            "templatecomment",
            "filename",
            "oldvalue",
            "newvalue",
        ],
    );
}

#[test]
fn rst_checks_document_prose_without_literals_references_or_directives() {
    assert_scope(
        "rst",
        "Heading prose\n=============\n\nParagraph prose with *emphasis prose*, ``inlinecode``, :py:func:`functionname`, targetref_ and |substitutionname|.\n\n- List prose\n\nTerm prose\n    Definition prose\n\n| Line prose\n\nLiteral example::\n\n    literalcode\n\n>>> doctestcode()\n\n.. code-block:: python\n\n   directivecode()\n\n.. _targetref: https://example.com/pathvalue\n",
        &[
            "Heading prose",
            "Paragraph prose",
            "emphasis prose",
            "List prose",
            "Term prose",
            "Definition prose",
            "Line prose",
            "Literal example",
        ],
        &[
            "inlinecode",
            "functionname",
            "targetref",
            "substitutionname",
            "literalcode",
            "doctestcode",
            "directivecode",
            "pathvalue",
        ],
    );
}

#[test]
fn typst_checks_markup_and_function_content_without_code_math_or_references() {
    assert_scope(
        "typst",
        "= Heading prose\n\nBody prose with *strong prose* and _emphasis prose_.\n#let variable = \"stringvalue\"\n#text[Content prose]\n`inlinecode`\n```rust\n// codecomment\nlet codevalue = 1;\n```\n$ mathvalue $\n<labelname> @labelname https://example.com/pathvalue\n\\u{0061}\n",
        &[
            "Heading prose",
            "Body prose",
            "strong prose",
            "emphasis prose",
            "Content prose",
        ],
        &[
            "variable",
            "stringvalue",
            "inlinecode",
            "codecomment",
            "codevalue",
            "mathvalue",
            "labelname",
            "pathvalue",
            "0061",
        ],
    );
}
