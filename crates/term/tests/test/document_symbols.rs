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
    // Let the real dynamic-query debounce publish its callbacks before idle.
    config.editor.idle_timeout = editor_config::Config::default().idle_timeout;
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

async fn wait_for_symbol(
    app: &mut term::application::TestApplication,
    name: &str,
) -> anyhow::Result<()> {
    // LSP jobs can finish after the idle pump returns. Wait for their picker callback to render.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        super::helpers::keys(app, "").await?;
        let screen: String = app
            .terminal_backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        if screen.contains(&format!("symbol:{name}")) {
            return Ok(());
        }
        assert!(tokio::time::Instant::now() < deadline, "{screen}");
    }
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
    wait_for_symbol(&mut app, "abcd").await?;
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
        wait_for_symbol(&mut app, name).await?;
    }
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn symbol_picker_reports_disabled_or_missing_language_servers() -> anyhow::Result<()> {
    for (enabled, expected) in [
        (false, "Language servers are disabled"),
        (
            true,
            "No configured language server supports document symbols",
        ),
    ] {
        let mut config = test_config();
        config.editor.lsp.enable = enabled;
        config.keys = toml::from_str("[normal]\ns = 'symbol_picker'")?;
        let mut app = AppBuilder::new().with_config(config).build()?;
        super::helpers::keys(&mut app, "s").await?;
        assert_eq!(app.editor.get_status().unwrap().0.as_ref(), expected);
        assert!(!super::helpers::screen(&app).contains("Document Symbols"));
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_loading_symbol_picker_cancels_its_request_and_reopening_restarts_it(
) -> anyhow::Result<()> {
    use std::{path::Path, time::Duration};

    async fn message(log: &Path, method: &str, index: usize) -> anyhow::Result<serde_json::Value> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(message) = lsp::read_log(log)?
                    .into_iter()
                    .filter(|message| message["method"] == method)
                    .nth(index)
                {
                    return anyhow::Ok(message);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?
    }

    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("initialize-ready"));
    let log = lsp::log_path(dir.path(), "symbols");
    let server = ServerConfig::feature("symbols", "--document-symbols", dir.path())
        .initialize_gate(&gate)
        .arg("--hold-symbols")
        .arg("--request-log")
        .arg(&log)
        .toml();
    let mut config = test_config();
    config.editor.lsp.enable = true;
    config.editor.breadcrumb.enable = true;
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

    let background = message(&log, "textDocument/documentSymbol", 0).await?;
    let mut previous = None;
    for (index, keys) in ["<space>s", "<space>'"].into_iter().enumerate() {
        super::helpers::keys(&mut app, keys).await?;
        let rendered = super::helpers::screen(&app);
        assert!(rendered.contains("Document Symbols"), "{rendered}");
        assert!(rendered.contains("Loading symbols..."), "{rendered}");
        let request = message(&log, "textDocument/documentSymbol", index + 1).await?;
        assert_ne!(request["id"], background["id"]);
        assert_ne!(Some(&request["id"]), previous.as_ref());
        super::helpers::keys(&mut app, "<esc>").await?;
        let cancel = message(&log, "$/cancelRequest", index).await?;
        assert_eq!(cancel["params"]["id"], request["id"]);
        assert!(!super::helpers::screen(&app).contains("Document Symbols"));
        previous = Some(request["id"].clone());
    }
    assert!(!lsp::read_log(&log)?.iter().any(|message| {
        message["method"] == "$/cancelRequest" && message["params"]["id"] == background["id"]
    }));
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
    assert_eq!(nested.symbol.location.range.end.character, 1);
    let flat = symbols
        .iter()
        .find(|symbol| symbol.symbol.name == "flat")
        .unwrap();
    assert!(flat.symbol.location.uri.to_string().ends_with(".other"));

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
