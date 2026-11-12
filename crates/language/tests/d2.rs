use editor_core::indent::IndentStyle;
use editor_core::Rope;
use language::{indent::treesitter_indent_for_pos, Syntax};
use stdx::rope::{RopeExt, RopeSliceExt};

#[test]
fn reindent_preserves_multiline_literal_whitespace() {
    for source in [
        "note: |\n  - outer\n    - inner\n|\n",
        "note: |md\n  - outer\n    - inner\n|\n",
        "note: |unknown\n  literal text\n    indented example\n|\n",
        "\"\"\"\n  multiline comment\n    indented example\n\"\"\"\n",
        "container: {\n  note: |\n    - outer\n      - inner\n  |\n}\n",
    ] {
        assert_reindent(source, source);
    }
}

#[test]
fn reindent_uses_embedded_python_indentation() {
    assert_reindent(
        "code: |py\n  def answer():\n        return 42\n|\n",
        "code: |py\n  def answer():\n    return 42\n|\n",
    );
}

fn assert_reindent(source: &str, expected: &str) {
    let loader = language::loader::default_lang_loader(loader::syntax::Resources::default());
    let language = loader.language_for_name("d2").unwrap();
    let doc = Rope::from_str(source);
    let expected = Rope::from_str(expected);
    let text = doc.slice(..);
    let syntax = Syntax::new(text, language, &loader).unwrap();
    let query = loader.indent_query(language).unwrap();
    let style = IndentStyle::Spaces(2);

    for line_idx in 0..doc.len_lines(stdx::rope::LINE_TYPE) {
        let line = text.line(line_idx, stdx::rope::LINE_TYPE);
        let Some(pos) = line.first_non_whitespace_char() else {
            continue;
        };
        let indent = treesitter_indent_for_pos(
            query,
            &syntax,
            &loader,
            2,
            2,
            text,
            line_idx,
            text.line_to_char_idx(line_idx) + pos,
            false,
        )
        .unwrap()
        .to_string(&style, 2);
        assert_eq!(
            indent,
            expected
                .line(line_idx, stdx::rope::LINE_TYPE)
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect::<String>(),
            "line {} of {source:?}",
            line_idx + 1,
        );
    }
}

#[test]
fn tags_named_nodes_but_not_connections() {
    let loader = language::loader::default_lang_loader(loader::syntax::Resources::default());
    let language = loader.language_for_name("d2").unwrap();
    let source = "api\ndatabase: cylinder\ncluster.worker\nnetwork: {\n  worker\n}\napi -> database: request\nrender(input string)\n";
    let text = Rope::from_str(source);
    let syntax = Syntax::new(text.slice(..), language, &loader).unwrap();
    let mut tags: Vec<_> = language::tags::tags(&syntax, &loader, text.slice(..), None)
        .map(|tag| (tag.kind.as_str(), tag.name))
        .collect();
    tags.sort_unstable();

    assert_eq!(
        tags,
        [
            ("class", "api".to_owned()),
            ("class", "cluster.worker".to_owned()),
            ("class", "database".to_owned()),
            ("class", "network".to_owned()),
            ("class", "worker".to_owned()),
            ("function", "render".to_owned()),
        ]
    );
}
