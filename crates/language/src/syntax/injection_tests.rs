//! Staged parsing with compiled-only languages.

use super::*;
use editor_core::{Rope, Transaction};

/// Exercises both injections a Rust buffer usually has: doc comments and format strings.
const RUST: &str = "/// Returns **one**.\nfn one() -> u8 {\n    println!(\"{}\", 1);\n    1\n}\n";

#[test]
fn json_query_prefilter_preserves_comments_overrides_and_edit_cleanup() {
    let reference_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(reference_dir.path().join("queries/json")).unwrap();
    std::fs::write(
        reference_dir.path().join("queries/json/injections.scm"),
        "((comment) @injection.content (#set! injection.language \"comment\"))",
    )
    .unwrap();
    let mut reference_paths = vec![reference_dir.path().to_owned()];
    reference_paths.extend(loader::runtime_dirs().iter().cloned());
    let reference = crate::loader::default_lang_loader(Resources::new(reference_paths));
    let loader = crate::loader::default_lang_loader(Resources::default());
    for loader in [&loader, &reference] {
        loader.set_scopes(
            ["comment", "string", "punctuation", "constant"]
                .map(String::from)
                .into(),
        );
    }
    let language = loader.language_for_name("json").unwrap();
    let reference_language = reference.language_for_name("json").unwrap();
    let mut before = Rope::from_str("{\"name\":\"value\"}");
    let mut syntax = Syntax::new(before.slice(..), language, &loader).unwrap();
    for source in [
        "{\"uri\":\"https://example.org\"}",
        "/* todo */ {\"name\":\"value\"}",
        "// note\n{\"name\":\"value\"}",
        "{\"name\":\"value\"}",
        "/* again */ {}",
    ] {
        let after = Rope::from_str(source);
        let transaction = Transaction::change(
            &before,
            [(0, before.len_chars(), Some(source.into()))].into_iter(),
        );
        syntax
            .update(
                before.slice(..),
                after.slice(..),
                transaction.changes(),
                &loader,
            )
            .unwrap();
        let fresh = Syntax::new(after.slice(..), language, &loader).unwrap();
        let original = Syntax::new(after.slice(..), reference_language, &reference).unwrap();
        assert_eq!(
            layer_languages(&syntax, &loader, &after),
            layer_languages(&fresh, &loader, &after)
        );
        assert_eq!(
            layer_languages(&syntax, &loader, &after),
            layer_languages(&original, &reference, &after)
        );
        assert_eq!(
            highlights(&syntax, &loader, &after),
            highlights(&fresh, &loader, &after)
        );
        assert_eq!(
            highlights(&syntax, &loader, &after),
            highlights(&original, &reference, &after)
        );
        before = after;
    }
}

fn rust_loader() -> (Loader, Language) {
    let loader = crate::loader::default_lang_loader(Resources::default());
    loader.set_scopes(
        [
            "comment",
            "function",
            "keyword",
            "markup",
            "punctuation",
            "string",
            "type",
        ]
        .map(String::from)
        .into(),
    );
    let rust = loader.language_for_name("rust").unwrap();
    (loader, rust)
}

fn is_compiled(loader: &Loader, name: &str) -> bool {
    let language = loader.language_for_name(name).unwrap();
    loader.language(language).syntax.get().is_some()
}

fn names(loader: &Loader, languages: &[Language]) -> Vec<String> {
    let mut names: Vec<_> = languages
        .iter()
        .map(|&language| loader.language(language).config().language_id.clone())
        .collect();
    names.sort();
    names
}

/// Languages of every layer, by byte, so missing or extra injections show up.
fn layer_languages(syntax: &Syntax, loader: &Loader, text: &Rope) -> Vec<Vec<String>> {
    (0..text.len() as u32)
        .map(|byte| {
            let layers: Vec<_> = syntax.layers_for_byte_range(byte, byte).collect();
            let languages: Vec<_> = layers.iter().map(|&l| syntax.layer(l).language).collect();
            names(loader, &languages)
        })
        .collect()
}

fn highlights(syntax: &Syntax, loader: &Loader, text: &Rope) -> Vec<(u32, Vec<u32>)> {
    let mut highlighter = syntax.highlighter(text.slice(..), loader, ..);
    let mut events = Vec::new();
    while highlighter.next_event_offset() != u32::MAX {
        let offset = highlighter.next_event_offset();
        let (_, highlights) = highlighter.advance();
        events.push((offset, highlights.map(|h| h.idx() as u32).collect()));
    }
    events
}

#[test]
fn first_parse_compiles_only_the_root_language() {
    let (loader, rust) = rust_loader();
    let text = Rope::from_str(RUST);
    let (syntax, missing) = Syntax::new_with_compiled(text.slice(..), rust, &loader).unwrap();

    assert_eq!(
        names(&loader, &missing),
        ["markdown-rustdoc", "rust-format-args-macro"]
    );
    assert!(is_compiled(&loader, "rust"));
    for name in [
        "markdown-rustdoc",
        "rust-format-args-macro",
        "markdown.inline",
    ] {
        assert!(!is_compiled(&loader, name), "{name} was compiled");
    }
    // The skipped injections are left out of the tree instead of becoming empty layers,
    // so highlighting the first tree with the full loader cannot compile them either.
    // Rust injects itself into macro arguments, so a byte can be in several Rust layers.
    assert!(layer_languages(&syntax, &loader, &text)
        .iter()
        .flatten()
        .all(|language| language == "rust"));
    assert!(!highlights(&syntax, &loader, &text).is_empty());
    assert!(!is_compiled(&loader, "markdown-rustdoc"));
}

#[test]
fn compiling_the_missing_languages_gives_the_same_tree_as_a_full_parse() {
    let text = Rope::from_str(RUST);
    let (staged, rust) = rust_loader();
    let (_, missing) = Syntax::new_with_compiled(text.slice(..), rust, &staged).unwrap();
    staged.compile_syntax_configs(&missing);
    assert!(is_compiled(&staged, "markdown-rustdoc"));
    assert!(is_compiled(&staged, "rust-format-args-macro"));
    let staged_syntax = Syntax::new(text.slice(..), rust, &staged).unwrap();

    let (full, _) = rust_loader();
    let full_syntax = Syntax::new(text.slice(..), rust, &full).unwrap();
    let layers = layer_languages(&full_syntax, &full, &text);
    assert!(layers
        .iter()
        .any(|layers| layers.contains(&"markdown.inline".into())));
    assert!(layers
        .iter()
        .any(|layers| layers.contains(&"rust-format-args".into())));
    assert_eq!(layer_languages(&staged_syntax, &staged, &text), layers);
    assert_eq!(
        highlights(&staged_syntax, &staged, &text),
        highlights(&full_syntax, &full, &text)
    );
}

#[test]
fn compiled_only_updates_skip_new_injections_until_a_normal_update() {
    let (loader, rust) = rust_loader();
    let old = Rope::from_str("fn one() {}\n");
    let (mut syntax, missing) = Syntax::new_with_compiled(old.slice(..), rust, &loader).unwrap();
    assert!(missing.is_empty());

    let doc_comment = Transaction::change(&old, [(0, 0, Some("/// **doc**\n".into()))].into_iter());
    let mut text = old.clone();
    assert!(doc_comment.apply(&mut text));
    syntax
        .update_compiled(
            old.slice(..),
            text.slice(..),
            doc_comment.changes(),
            &loader,
        )
        .unwrap();
    assert!(!is_compiled(&loader, "markdown-rustdoc"));
    assert_eq!(syntax.tree().root_node().byte_range(), 0..text.len() as u32);

    let space = Transaction::change(&text, [(0, 0, Some(" ".into()))].into_iter());
    let mut spaced = text.clone();
    assert!(space.apply(&mut spaced));
    syntax
        .update(text.slice(..), spaced.slice(..), space.changes(), &loader)
        .unwrap();
    assert!(is_compiled(&loader, "markdown-rustdoc"));
}

#[test]
fn compiling_several_languages_fills_each_cache_once() {
    let (loader, _) = rust_loader();
    let languages: Vec<_> = ["json", "toml", "comment", "json"]
        .into_iter()
        .map(|name| loader.language_for_name(name).unwrap())
        .collect();
    loader.compile_syntax_configs(&languages);
    for name in ["json", "toml", "comment"] {
        assert!(is_compiled(&loader, name), "{name} was not compiled");
    }
    loader.compile_syntax_configs(&[]);
}
