use super::*;

fn configuration() -> Configuration {
    toml::from_str(
        r#"
        [[language]]
        name = "resource-json"
        grammar = "json"
        scope = "source.resource-json"
        file-types = ["resource-json"]
    "#,
    )
    .unwrap()
}

fn resources(dir: &Path) -> Resources {
    let mut paths = vec![dir.to_owned()];
    paths.extend_from_slice(loader::runtime_dirs());
    Resources::new(paths)
}

fn write(dir: &Path, filename: &str, text: &str) {
    let query_dir = dir.join("queries/resource-json");
    std::fs::create_dir_all(&query_dir).unwrap();
    std::fs::write(query_dir.join(filename), text).unwrap();
}

#[test]
fn compilers_use_supplied_sources_and_preserve_query_specific_validation() {
    use queries::*;
    let grammar = Resources::default()
        .grammar("json")
        .unwrap()
        .expect("json grammar");
    let scopes = vec!["string".into()];
    let config =
        compile_syntax_config(grammar, "supplied", "(string) @string", "", "", &scopes).unwrap();
    assert_eq!(config.grammar, grammar);
    assert!(
        compile_indent_query(grammar, "supplied", "(object) @indent")
            .unwrap()
            .is_some()
    );
    assert!(
        compile_textobject_query(grammar, "supplied", "(string) @entry.inside")
            .unwrap()
            .is_some()
    );
    assert!(compile_tag_query(grammar, "supplied", "(string) @name")
        .unwrap()
        .is_some());
    assert!(
        compile_rainbow_query(grammar, "supplied", "(object) @rainbow.scope")
            .unwrap()
            .is_some()
    );
    assert!(
        compile_spellcheck_query(grammar, "supplied", "(string) @spell")
            .unwrap()
            .is_some()
    );
    assert!(compile_indent_query(grammar, "supplied", "")
        .unwrap()
        .is_none());
    assert!(compile_textobject_query(grammar, "supplied", "")
        .unwrap()
        .is_none());
    assert!(compile_tag_query(grammar, "supplied", "")
        .unwrap()
        .is_none());
    assert!(compile_rainbow_query(grammar, "supplied", "")
        .unwrap()
        .is_none());
    assert!(compile_spellcheck_query(grammar, "supplied", "")
        .unwrap()
        .is_none());
    // Keep the feature-specific predicate rules as well as tree-sitter parsing.
    let invalid = "((string) @name (#unknown!))";
    assert!(compile_tag_query(grammar, "supplied", invalid).is_err());
    assert!(compile_rainbow_query(grammar, "supplied", invalid).is_err());
    assert!(compile_spellcheck_query(grammar, "supplied", "(missing_node) @spell").is_err());
}

#[test]
fn resources_are_lazy_and_compiled_caches_belong_to_each_loader() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let a = Loader::new(configuration(), resources(first.path())).unwrap();
    let b = Loader::new(configuration(), resources(second.path())).unwrap();
    let language = a.language_for_name("resource-json").unwrap();
    assert!(a.language(language).syntax.get().is_none());
    // Files created after construction are read on first use.
    write(first.path(), "highlights.scm", "(string) @string");
    write(second.path(), "highlights.scm", "(invalid_node) @string");
    assert!(a.get_config(language).is_some());
    assert!(b.get_config(language).is_none());
    write(first.path(), "highlights.scm", "(invalid_node) @string");
    write(second.path(), "highlights.scm", "(string) @string");
    // Successful and failed compilations are cached until a new loader is installed.
    assert!(a.get_config(language).is_some());
    assert!(b.get_config(language).is_none());
    let fresh_a = Loader::new(configuration(), a.resources().clone()).unwrap();
    let fresh_b = Loader::new(configuration(), b.resources().clone()).unwrap();
    assert!(fresh_a.get_config(language).is_none());
    assert!(fresh_b.get_config(language).is_some());
}

#[test]
fn validation_reports_each_query_error_without_populating_editor_caches() {
    let dir = tempfile::tempdir().unwrap();
    let loader = Loader::new(configuration(), resources(dir.path())).unwrap();
    let language = loader.language_for_name("resource-json").unwrap();
    loader.validate_queries(language).unwrap();
    for filename in [
        "highlights.scm",
        "injections.scm",
        "locals.scm",
        "indents.scm",
        "textobjects.scm",
        "tags.scm",
        "rainbows.scm",
        "spellcheck.scm",
    ] {
        write(dir.path(), filename, "(invalid_node) @invalid");
        let error = loader.validate_queries(language).unwrap_err().to_string();
        assert!(error.contains("resource-json"), "{filename}: {error}");
        // Tree-house compiles highlights, injections, and locals as one syntax config.
        assert!(
            error.contains(
                if matches!(filename, "highlights.scm" | "injections.scm" | "locals.scm") {
                    "highlights"
                } else {
                    filename
                }
            ),
            "{error}"
        );
        write(dir.path(), filename, "");
        loader.validate_queries(language).unwrap();
    }
    assert!(loader.language(language).syntax.get().is_none());
    assert!(loader.language(language).indent_query.get().is_none());
}

#[test]
fn missing_grammar_stays_optional_even_with_invalid_query_files() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "highlights.scm", "(invalid_node)");
    let loader = Loader::new(configuration(), Resources::new(vec![dir.path().into()])).unwrap();
    let language = loader.language_for_name("resource-json").unwrap();
    assert!(loader.get_config(language).is_none());
    loader.validate_queries(language).unwrap();
}
