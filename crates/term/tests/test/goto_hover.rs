//! `goto_hover` writes hover docs into a Markdown buffer, while `hover` keeps the popup.
use std::{path::Path, time::Duration};

use term::application::Application;
use view::{current_ref, editor::Action};

use super::helpers::{
    keys,
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

async fn app(dir: &Path) -> anyhow::Result<Application> {
    let gate = Gate::new(dir.join("initialize-ready"));
    let servers = ServerConfig::feature("alpha", "--language-features", dir)
        .initialize_gate(&gate)
        .toml();
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "hover-test",
            "hover-test",
            &["alpha"],
            &servers,
        ))
        .build()?;
    app.editor
        .open(&dir.join("document.hover-test"), Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    Ok(app)
}

fn current_path(app: &Application) -> Option<std::path::PathBuf> {
    let (_, doc) = current_ref!(app.editor);
    doc.path().map(Path::to_path_buf)
}

/// Settle the event loop until the focused document is the hover buffer.
async fn wait_for_scratch(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_path(app).is_some() {
            keys(app, "").await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        anyhow::Ok(())
    })
    .await?
}

fn screen(app: &Application) -> String {
    let buffer = app.terminal_backend().buffer();
    buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_hover_opens_the_docs_in_a_markdown_buffer_and_jumps_back() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("document.hover-test");
    std::fs::write(&source, "fn original() {}\n")?;
    let mut app = app(dir.path()).await?;
    let source = current_path(&app).expect("source file is open");

    keys(&mut app, "<space>K").await?;
    wait_for_scratch(&mut app).await?;
    {
        let (_, doc) = current_ref!(app.editor);
        assert_eq!(doc.text().to_string(), "alpha: fn original() {}\n");
        assert_eq!(doc.language_name(), Some("markdown"));
        assert!(!doc.is_modified(), "the hover buffer is not a pending edit");
    }
    keys(&mut app, "<C-o>").await?;
    assert_eq!(current_path(&app), Some(source.clone()));

    // `hover` still shows a popup and keeps the source focused.
    keys(&mut app, "<space>k").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !screen(&app).contains("alpha: fn original") {
            keys(&mut app, "").await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert_eq!(current_path(&app), Some(source));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_hover_without_results_reports_and_stays_in_place() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // The fake server answers `null` for documents containing "empty".
    std::fs::write(dir.path().join("document.hover-test"), "empty\n")?;
    let mut app = app(dir.path()).await?;
    let source = current_path(&app);

    keys(&mut app, "<space>K").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !screen(&app).contains("No hover results available.") {
            keys(&mut app, "").await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert_eq!(current_path(&app), source);
    assert_eq!(app.editor.documents().count(), 1);
    Ok(())
}
