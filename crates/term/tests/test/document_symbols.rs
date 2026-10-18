use editor_core::Transaction;
use view::{current, current_ref, editor::Action};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

#[tokio::test(flavor = "multi_thread")]
async fn document_symbol_queries_preserve_hierarchy_locations_and_origin() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("initialize-ready"));
    let servers = format!(
        "{}{}{}",
        ServerConfig::feature("nested", "--document-symbols", dir.path())
            .initialize_gate(&gate)
            .toml(),
        ServerConfig::feature("flat", "--document-symbols", dir.path())
            .initialize_gate(&gate)
            .arg("--flat-symbols")
            .toml(),
        ServerConfig::feature("failing", "--document-symbols", dir.path())
            .initialize_gate(&gate)
            .arg("--fail-symbols")
            .toml()
    );
    let mut config = test_config();
    config.editor.lsp.enable = true;
    config.editor.breadcrumb.enable = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "symbols-test",
            "symbols-test",
            &["nested", "flat", "failing"],
            &servers,
        ))
        .build()?;
    let path = dir.path().join("document.symbols-test");
    std::fs::write(&path, "abcd\n")?;
    app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 3).await?;
    let (view, doc) = current_ref!(app.editor);
    let (document, view) = (doc.id(), view.id);
    let symbols = app
        .editor
        .document_symbols(document, view)?
        .run()
        .await
        .unwrap()
        .into_symbols(&app.editor)
        .unwrap();
    assert_eq!(symbols.len(), 2);
    let nested = symbols
        .iter()
        .find(|symbol| symbol.symbol.name == "nested")
        .unwrap();
    assert_eq!(nested.children[0].symbol.name, "child");
    assert_eq!(nested.location.range.end.character, 1);
    let flat = symbols
        .iter()
        .find(|symbol| symbol.symbol.name == "flat")
        .unwrap();
    assert!(flat.location.uri.to_string().ends_with(".other"));
    assert_eq!(
        flat.location.uri.to_url().unwrap(),
        flat.symbol.location.uri
    );

    let replaced = app
        .editor
        .document_symbols(document, view)?
        .run()
        .await
        .unwrap();
    let latest = app.editor.document_symbols(document, view)?;
    assert!(replaced.into_symbols(&app.editor).is_none());
    let queued = latest.run().await.unwrap();
    let (active_view, doc) = current!(app.editor);
    let transaction =
        Transaction::insert(doc.text(), doc.selection(active_view.id), "typed".into());
    assert!(view::editing::apply(doc, &transaction, active_view.id));
    assert!(queued.into_symbols(&app.editor).is_none());

    let queued = app
        .editor
        .document_symbols(document, view)?
        .run()
        .await
        .unwrap();
    app.editor.new_file(Action::HorizontalSplit);
    assert!(queued.into_symbols(&app.editor).is_none());
    Ok(())
}
