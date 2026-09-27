//! Exercise shared range formatting against the real LSP transport.
use editor_core::Selection;
use term::application::Application;
use view::{current, current_ref, editor::Action, features::formatting};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

async fn application(dir: &std::path::Path) -> anyhow::Result<Application> {
    let gate = Gate::new(dir.join("initialize-ready"));
    let server = ServerConfig::feature("alpha", "--formatting", dir).initialize_gate(&gate);
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "formatting-test",
            "formatting-test",
            &["alpha"],
            &server.toml(),
        ))
        .build()?;
    let path = dir.join("document.formatting-test");
    std::fs::write(&path, "😀 foo bar\n")?;
    app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::single(2, 5));
    Ok(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn range_formatting_converts_utf16_positions_and_preserves_surrounding_text(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = application(dir.path()).await?;
    let (view, doc) = current!(app.editor);
    doc.indent_style = editor_core::indent::IndentStyle::Spaces(2);
    assert_ne!(doc.tab_width(), doc.indent_width());
    let view = view.id;
    let update = formatting::prepare_selections(&app.editor, view)?
        .run()
        .await;
    assert!(update.apply(&mut app.editor)?);
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "😀 formatted bar\n"
    );
    let log = std::fs::read_to_string(lsp::log_path(dir.path(), "alpha"))?;
    let request = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|message| message["method"] == "textDocument/rangeFormatting")
        .unwrap();
    assert_eq!(request["params"]["options"]["tabSize"], 2);
    assert_eq!(request["params"]["range"]["start"]["character"], 3);
    assert_eq!(request["params"]["range"]["end"]["character"], 6);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn range_formatting_rejects_rebound_views_and_detached_servers() -> anyhow::Result<()> {
    for rebind in [true, false] {
        let dir = tempfile::tempdir()?;
        let mut app = application(dir.path()).await?;
        let (view, doc) = current_ref!(app.editor);
        let (view, id) = (view.id, doc.id());
        let server = doc.language_servers().next().unwrap().id();
        let update = formatting::prepare_selections(&app.editor, view)?
            .run()
            .await;
        if rebind {
            app.editor.new_file(Action::Replace);
            app.editor.switch(id, Action::Replace);
        } else {
            app.editor.handle_language_server_exit(server);
        }
        assert!(!update.apply(&mut app.editor)?);
        assert_eq!(
            app.editor.document(id).unwrap().text().to_string(),
            "😀 foo bar\n"
        );
        assert!(app.close().await.is_empty());
    }
    Ok(())
}
