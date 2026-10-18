//! Workspace symbol sessions keep their original providers and release canceled requests.
use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};
use anyhow::Context as _;
use std::{path::Path, time::Duration};
use term::application::Application;
use view::{current_ref, editor::Action};

async fn fixture(dir: &Path, args: &[&str]) -> anyhow::Result<Application> {
    let initialize = Gate::new(dir.join("initialize"));
    let mut server =
        ServerConfig::feature("symbols", "--workspace-symbols", dir).initialize_gate(&initialize);
    for arg in args {
        server = server.arg(arg);
    }
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "symbols",
            "symbols",
            &["symbols"],
            &server.toml(),
        ))
        .build()?;
    let path = dir.join("document.symbols");
    std::fs::write(&path, "😀 foo\n")?;
    app.editor.open(&path, Action::Replace)?;
    initialize.release()?;
    lsp::initialize(&mut app, 1).await?;
    Ok(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn symbol_queries_use_the_originating_providers_after_focus_moves() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &[]).await?;
    let document = current_ref!(app.editor).1.id();
    let session = app.editor.workspace_symbols(document)?;
    app.editor.new_file(Action::Replace);
    let mut task = session.query(&app.editor, "foo".into())?;
    let items = tokio::time::timeout(Duration::from_secs(5), task.next_batch())
        .await?
        .context("missing symbols")??;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].symbol.name, "symbols:foo");
    assert_eq!(
        items[0].location.offset_encoding,
        lsp_client::OffsetEncoding::Utf16
    );
    assert_eq!(
        items[0].location.uri,
        app.editor.document(document).unwrap().uri().unwrap()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_symbol_query_cancels_its_server_request() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--hold-symbols"]).await?;
    let document = current_ref!(app.editor).1.id();
    let session = app.editor.workspace_symbols(document)?;
    let task = session.query(&app.editor, "pending".into())?;
    wait_message(dir.path(), "workspace/symbol").await?;
    drop(task);
    wait_message(dir.path(), "$/cancelRequest").await?;
    app.editor.new_file(Action::Replace);
    assert!(app.editor.close_document(document, true).is_ok());
    assert!(!session.is_current(&app.editor));
    assert!(session.query(&app.editor, "closed".into()).is_err());
    Ok(())
}

async fn wait_message(dir: &Path, method: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if lsp::read_log(&lsp::log_path(dir, "symbols"))?
                .iter()
                .any(|message| message["method"] == method)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_symbol_picker_cancels_pending_queries() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--hold-symbols"]).await?;
    super::helpers::test_key_sequences(
        &mut app,
        vec![(Some("<space>S"), None), (Some("<esc>"), None)],
        false,
    )
    .await?;
    wait_message(dir.path(), "$/cancelRequest").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_symbol_picker_refreshes_edited_buffers_and_keeps_its_query() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--symbols-from-text"]).await?;
    super::helpers::keys(&mut app, "<space>Sfoo").await?;
    let (view, doc) = view::current!(app.editor);
    let edit = editor_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("new\n".into()))].into_iter(),
    );
    assert!(view::editing::apply(doc, &edit, view.id));
    super::helpers::keys(&mut app, "").await?;
    let screen: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("symbols:new:foo"), "{screen}");
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn revisiting_a_picker_query_restarts_its_canceled_request() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--hold-symbols"]).await?;
    super::helpers::test_key_sequences(
        &mut app,
        vec![
            (Some("<space>S"), None),
            (Some("fo<backspace><backspace>"), None),
        ],
        false,
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let queries: Vec<_> = lsp::read_log(&lsp::log_path(dir.path(), "symbols"))?
                .into_iter()
                .filter(|message| message["method"] == "workspace/symbol")
                .collect();
            if queries.len() >= 2 {
                assert_eq!(queries.last().unwrap()["params"]["query"], "");
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await??;
    Ok(())
}
