use editor_core::Rope;
use language::{Loader, Syntax};

fn loader() -> Loader {
    language::loader::default_lang_loader(loader::syntax::Resources::default())
}

#[test]
fn workspaces_have_one_symbol_with_a_name_or_keyword_fallback() {
    let loader = loader();
    let language = loader.language_for_name("structurizr").unwrap();
    for (source, expected) in [
        ("workspace {}", "workspace"),
        ("workspace \"Example\" {}", "\"Example\""),
        ("workspace \"Example\" \"Description\" {}", "\"Example\""),
        ("workspace extends \"base.dsl\" {}", "workspace"),
    ] {
        let text = Rope::from_str(source);
        let syntax = Syntax::new(text.slice(..), language, &loader).unwrap();
        let names: Vec<_> = language::tags::tags(&syntax, &loader, text.slice(..), None)
            .map(|tag| tag.name)
            .collect();
        assert_eq!(names, [expected], "{source}");
    }
}

#[test]
fn around_textobjects_include_headers_and_preserve_sibling_boundaries() {
    let loader = loader();
    let language = loader.language_for_name("structurizr").unwrap();
    let person = "user = person \"User\" {\n    description \"A user\"\n}";
    let properties = "properties {\n    \"key\" \"value\"\n}";
    let system = format!("system = softwareSystem \"System\" {{\n    {properties}\n}}");
    let model = format!("model {{\n    {person}\n    {system}\n}}");
    let source = format!("workspace \"Example\" {{\n    {model}\n}}");
    let text = Rope::from_str(&source);
    let syntax = Syntax::new(text.slice(..), language, &loader).unwrap();
    let query = loader.textobject_query(language).unwrap();
    for (capture, expected) in [
        ("class.around", vec![source.as_str(), model.as_str()]),
        ("function.around", vec![person, system.as_str(), properties]),
    ] {
        let spans: Vec<_> = query
            .capture_nodes(capture, &syntax.tree().root_node(), text.slice(..))
            .unwrap()
            .map(|node| &source[node.byte_range()])
            .collect();
        assert_eq!(spans, expected, "{capture}");
    }
}
