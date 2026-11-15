//! Loading menus and prompts cancel their own requests when dismissed.

use std::{path::Path, time::Duration};

use anyhow::Context as _;
use editor_core::Selection;
use term::application::TestApplication;
use view::{current, current_ref, editor::Action};

use super::helpers::{
    keys,
    lsp::{self, Gate, ServerConfig},
    screen, test_config, AppBuilder,
};

async fn application(dir: &Path, mode: &str, options: &[&str]) -> anyhow::Result<TestApplication> {
    let gate = Gate::new(dir.join("initialize-ready"));
    let mut server = ServerConfig::feature("interaction", mode, dir).initialize_gate(&gate);
    for option in options {
        server = server.arg(option);
    }
    if mode == "--code-lenses" {
        server = server.arg("--per-line");
    }
    let mut config = test_config();
    config.editor.lsp.enable = true;
    config.editor.breadcrumb.enable = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "interaction",
            "interaction",
            &["interaction"],
            &server.toml(),
        ))
        .build()?;
    let path = dir.join("document.interaction");
    std::fs::write(&path, "😀 foo\n")?;
    app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 1)
        .await
        .with_context(|| format!("initialize {mode}"))?;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::point(3));
    Ok(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn dismissing_loading_lsp_menus_and_prompts_cancels_their_requests() -> anyhow::Result<()> {
    for (mode, input, method, message) in [
        (
            "--action-execution",
            "<space>a",
            "textDocument/codeAction",
            "Loading code actions...",
        ),
        (
            "--code-lenses",
            "<space>l",
            "codeLens/resolve",
            "Loading code lenses...",
        ),
        (
            "--rename",
            "<space>r",
            "textDocument/prepareRename",
            "Preparing rename...",
        ),
    ] {
        let dir = tempfile::tempdir()?;
        let log = lsp::log_path(dir.path(), "interaction");
        let mut app = application(dir.path(), mode, &["--hold-request", method]).await?;
        let background = if mode == "--code-lenses" {
            tokio::time::timeout(Duration::from_secs(5), async {
                while current_ref!(app.editor).1.code_lenses().is_empty() {
                    keys(&mut app, "").await?;
                }
                anyhow::Ok(())
            })
            .await
            .with_context(|| format!("discover lenses for {mode}"))??;
            Some(lsp::wait_message(&log, method).await?)
        } else {
            None
        };
        keys(&mut app, input).await?;
        let rendered = screen(&app);
        assert!(rendered.contains(message), "{mode}: {rendered}");
        let request = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(request) = lsp::read_log(&log)?.into_iter().find(|request| {
                    request["method"] == method
                        && background
                            .as_ref()
                            .is_none_or(|background| request["id"] != background["id"])
                }) {
                    return anyhow::Ok(request);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .with_context(|| format!("request {method}"))??;
        keys(&mut app, "<esc>").await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if lsp::read_log(&log)?.iter().any(|message| {
                    message["method"] == "$/cancelRequest"
                        && message["params"]["id"] == request["id"]
                }) {
                    return anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .with_context(|| format!("cancel {method}"))??;
        assert!(!screen(&app).contains(message), "{mode}");
        if let Some(background) = background {
            assert!(!lsp::read_log(&log)?.iter().any(|message| {
                message["method"] == "$/cancelRequest"
                    && message["params"]["id"] == background["id"]
            }));
        }
        assert_eq!(current_ref!(app.editor).1.text().to_string(), "😀 foo\n");
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn input_typed_during_rename_preparation_reaches_its_prompt() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("prepare-ready"));
    let mut app = application(
        dir.path(),
        "--rename",
        &["--prepare-gate", gate.path().to_str().unwrap()],
    )
    .await?;
    keys(&mut app, "<space>r<C-u>replacement").await?;
    assert!(screen(&app).contains("Preparing rename..."));
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "😀 foo\n");
    gate.release()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            keys(&mut app, "").await?;
            let rendered = screen(&app);
            if rendered.contains("rename-to:") && rendered.contains("replacement") {
                return anyhow::Ok(());
            }
        }
    })
    .await
    .with_context(|| format!("rename prompt did not become ready: {}", screen(&app)))??;
    let rendered = screen(&app);
    assert!(rendered.contains("rename-to:replacement"), "{rendered}");
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "😀 foo\n");
    keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}
