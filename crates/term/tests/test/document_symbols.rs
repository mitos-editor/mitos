use editor_core::Transaction;
use view::{current, current_ref, editor::Action};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

async fn syntax_picker_refreshes(keys: &str) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.rs");
    std::fs::write(&path, "fn sample_original() {}\n")?;
    let mut config = test_config();
    config.editor.lsp.enable = false;
    let mut app = AppBuilder::new().with_config(config).build()?;
    app.editor.open(&path, Action::Replace)?;
    super::helpers::keys(&mut app, keys).await?;
    let (view, doc) = current!(app.editor);
    let edit = Transaction::change(
        doc.text(),
        [(
            0,
            doc.text().len_chars(),
            Some("fn sample_updated() {}\n".into()),
        )]
        .into_iter(),
    );
    view::editing::apply(doc, &edit, view.id).unwrap();
    super::helpers::keys(&mut app, "").await?;
    let buffer = app.terminal_backend().buffer();
    let picker: String = buffer
        .content
        .chunks(buffer.area.width as usize)
        .flat_map(|row| row[..row.len() / 2].iter().map(|cell| cell.symbol()))
        .collect();
    assert!(picker.contains("sample_updated"), "{picker}");
    assert!(!picker.contains("sample_original"), "{picker}");
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_document_symbol_picker_follows_edits() -> anyhow::Result<()> {
    syntax_picker_refreshes("<space>s").await
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_workspace_symbol_picker_follows_edits() -> anyhow::Result<()> {
    syntax_picker_refreshes("<space>Ssample").await
}

#[tokio::test(flavor = "multi_thread")]
async fn document_symbol_picker_refreshes_after_edits_and_when_reopened() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("initialize-ready"));
    let server = ServerConfig::feature("symbols", "--document-symbols", dir.path())
        .initialize_gate(&gate)
        .arg("--symbols-from-text")
        .toml();
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "symbols-test",
            "symbols-test",
            &["symbols"],
            &server,
        ))
        .build()?;
    let path = dir.path().join("document.symbols-test");
    std::fs::write(&path, "abcd\n")?;
    app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    super::helpers::keys(&mut app, "<space>s").await?;
    for (name, reopen) in [("updated", false), ("reopened", true)] {
        if reopen {
            super::helpers::keys(&mut app, "<esc>").await?;
        }
        let (view, doc) = current!(app.editor);
        let edit = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(format!("{name}\n").into()))].into_iter(),
        );
        view::editing::apply(doc, &edit, view.id).unwrap();
        super::helpers::keys(&mut app, if reopen { "<space>'" } else { "" }).await?;
        super::helpers::keys(&mut app, "<home>").await?;
        let screen: String = app
            .terminal_backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains(&format!("symbol:{name}")), "{screen}");
    }
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

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
        stdx::Url::parse(flat.symbol.location.uri.as_str()).unwrap()
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
    view::editing::apply(doc, &transaction, active_view.id).unwrap();
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
