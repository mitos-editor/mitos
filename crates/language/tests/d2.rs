use editor_core::Rope;
use language::Syntax;

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
