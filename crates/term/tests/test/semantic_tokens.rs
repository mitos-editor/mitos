use editor_core::Transaction;
use term::application::TestApplication;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current,
    features::semantic_tokens::{self, SemanticTokensHandler},
    DocumentId,
};

use super::helpers::{
    callbacks,
    lsp::{self, ServerConfig},
    test_config,
};

struct Fixture {
    app: TestApplication,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    doc: DocumentId,
    log: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(range_only: bool) -> anyhow::Result<Self> {
        Self::with_args(range_only, &[]).await
    }

    async fn with_args(range_only: bool, args: &[&str]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let (mut app, callbacks, gate) = lsp::feature_app(
            dir.path(),
            "😀 foo\n",
            "semantic-test",
            &["semantic"],
            test_config(),
            |name| {
                let mut server = ServerConfig::feature(name, "--semantic-tokens", dir.path());
                for arg in args {
                    server = server.arg(arg);
                }
                if range_only {
                    server.arg("--range-only")
                } else {
                    server
                }
            },
            |handlers, sender, _| handlers.semantic_tokens = SemanticTokensHandler::new(sender),
        )?;
        // Keep the original fixture's initial cursor position.
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, editor_core::Selection::point(0));
        let doc = doc.id();
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        Ok(Self {
            app,
            callbacks,
            doc,
            log: lsp::log_path(dir.path(), "semantic"),
            _dir: dir,
        })
    }
    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        callbacks::response(&mut self.app, &mut self.callbacks).await
    }
    fn edit(&mut self, text: &str) {
        let (view, doc) = current!(self.app.editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        view::editing::apply(doc, &transaction, view.id).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn full_and_range_only_servers_refine_highlights_using_negotiated_utf16() -> anyhow::Result<()>
{
    for (range_only, args) in [
        (false, &[][..]),
        (true, &[][..]),
        (true, &["--range-options"][..]),
    ] {
        let mut f = Fixture::with_args(range_only, args).await?;
        let response = f.response().await?;
        response(&mut f.app.editor);
        let tokens = f.app.editor.document(f.doc).unwrap().semantic_tokens();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].range, 2..5);
        assert_eq!(tokens[0].scope, "function.method.definition.deprecated");
        let records = lsp::read_log(&f.log)?;
        let initialize = records
            .iter()
            .find(|m| m["method"] == "initialize")
            .unwrap();
        assert_eq!(
            initialize["params"]["capabilities"]["textDocument"]["semanticTokens"]["requests"]
                ["full"],
            serde_json::json!({"delta": true})
        );
        let method = if range_only {
            "textDocument/semanticTokens/range"
        } else {
            "textDocument/semanticTokens/full"
        };
        assert!(records.iter().any(|m| m["method"] == method));
        f.edit("😀 no tokens\n");
        assert!(f
            .app
            .editor
            .document(f.doc)
            .unwrap()
            .semantic_tokens()
            .is_empty());
        let response = f.response().await?;
        response(&mut f.app.editor);
        assert!(f
            .app
            .editor
            .document(f.doc)
            .unwrap()
            .semantic_tokens()
            .is_empty());
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_semantic_responses_cannot_cross_edits_or_configuration_changes(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(false).await?;
    let queued = f.response().await?;
    f.edit("😀 bar\n");
    queued(&mut f.app.editor);
    assert!(f
        .app
        .editor
        .document(f.doc)
        .unwrap()
        .semantic_tokens()
        .is_empty());
    let response = f.response().await?;
    response(&mut f.app.editor);
    let server = f
        .app
        .editor
        .document(f.doc)
        .unwrap()
        .language_servers_with_feature(language_config::LanguageServerFeature::SemanticTokens)
        .next()
        .unwrap()
        .id();
    semantic_tokens::refresh_for_language_server(&mut f.app.editor, server);
    let queued = f.response().await?;
    let mut config = f.app.editor.config().clone();
    config.lsp.semantic_tokens.enable = false;
    f.app
        .apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(
            config.clone(),
        )));
    queued(&mut f.app.editor);
    assert!(f
        .app
        .editor
        .document(f.doc)
        .unwrap()
        .semantic_tokens()
        .is_empty());
    config.lsp.semantic_tokens.enable = true;
    f.app
        .apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(config)));
    let response = f.response().await?;
    response(&mut f.app.editor);
    assert_eq!(
        f.app
            .editor
            .document(f.doc)
            .unwrap()
            .semantic_tokens()
            .len(),
        1
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn config_reload_requests_semantic_tokens_again() -> anyhow::Result<()> {
    let Some(root) = super::helpers::isolation::workspace(
        "test::semantic_tokens::config_reload_requests_semantic_tokens_again",
    )?
    else {
        return Ok(());
    };
    let mut f = Fixture::new(false).await?;
    let response = f.response().await?;
    response(&mut f.app.editor);
    assert_eq!(
        f.app
            .editor
            .document(f.doc)
            .unwrap()
            .semantic_tokens()
            .len(),
        1
    );

    std::fs::write(
        loader::lang_config_file(),
        format!("{}\n[[language]]\nname = 'semantic-test'\nscope = 'source.semantic-test'\nfile-types = ['semantic-test']\nroots = []\nlanguage-servers = ['semantic']\n",
            ServerConfig::feature("semantic", "--semantic-tokens", &root).toml()),
    )?;
    f.app
        .handle_config_events(view::editor::ConfigEvent::Refresh);
    assert_eq!(f.app.editor.get_status().unwrap().0, "Config refreshed");
    let response = f.response().await?;
    response(&mut f.app.editor);
    assert_eq!(
        f.app.editor.document(f.doc).unwrap().semantic_tokens()[0].range,
        2..5
    );
    assert_eq!(semantic_requests(&f)?.len(), 2);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_publication_invalidates_an_already_rendered_viewport() -> anyhow::Result<()> {
    let mut f = Fixture::new(false).await?;
    let response = f.response().await?;
    f.app
        .editor
        .set_theme(theme::Theme::from(toml::Value::Table(toml::toml! {
            "ui.text" = "red"
            "ui.selection" = { bg = "black" }
            "function.method" = "blue"
            "*.definition" = { modifiers = ["bold"] }
            "*.deprecated" = { modifiers = ["crossed_out"] }
        })))?;
    async fn color(app: &mut TestApplication) -> theme::Color {
        app.handle_editor_event(view::editor::EditorEvent::Redraw)
            .await;
        super::helpers::run_event_loop_until_idle(app).await;
        app.terminal_backend()
            .buffer()
            .content()
            .windows(3)
            .find(|cells| {
                cells[0].symbol() == "f" && cells[1].symbol() == "o" && cells[2].symbol() == "o"
            })
            .expect("document text is rendered")[0]
            .fg
    }
    assert_eq!(color(&mut f.app).await, theme::Color::Red);
    let before = f
        .app
        .editor
        .document(f.doc)
        .unwrap()
        .semantic_tokens_revision();
    response(&mut f.app.editor);
    assert_ne!(
        f.app
            .editor
            .document(f.doc)
            .unwrap()
            .semantic_tokens_revision(),
        before
    );
    assert_eq!(color(&mut f.app).await, theme::Color::Blue);

    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("😀".into()))].into_iter());
    view::editing::apply(doc, &transaction, view.id)?;
    assert_eq!(
        doc.semantic_tokens()
            .first()
            .map(|token| token.range.clone()),
        Some(3..6)
    );
    assert_eq!(color(&mut f.app).await, theme::Color::Blue);

    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::change(doc.text(), [(3, 3, Some("x".into()))].into_iter());
    view::editing::apply(doc, &transaction, view.id)?;
    assert!(doc.semantic_tokens().is_empty());
    assert_eq!(color(&mut f.app).await, theme::Color::Red);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_deltas_use_the_last_published_result_across_edits() -> anyhow::Result<()> {
    let mut f = Fixture::with_args(false, &["--deltas"]).await?;
    let response = f.response().await?;
    response(&mut f.app.editor);
    f.edit("😀 x\n");
    let response = f.response().await?;
    response(&mut f.app.editor);
    assert_eq!(
        f.app.editor.document(f.doc).unwrap().semantic_tokens()[0].range,
        2..3
    );
    f.edit("😀 foo\n");
    let response = f.response().await?;
    response(&mut f.app.editor);
    assert_eq!(
        f.app.editor.document(f.doc).unwrap().semantic_tokens()[0].range,
        2..5
    );
    let requests = semantic_requests(&f)?;
    assert_eq!(
        requests
            .iter()
            .map(|m| m["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "textDocument/semanticTokens/full",
            "textDocument/semanticTokens/full/delta",
            "textDocument/semanticTokens/full/delta"
        ]
    );
    assert_eq!(requests[1]["params"]["previousResultId"], "full-1");
    assert_eq!(requests[2]["params"]["previousResultId"], "delta-2");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

fn semantic_requests(f: &Fixture) -> anyhow::Result<Vec<serde_json::Value>> {
    Ok(lsp::read_log(&f.log)?
        .into_iter()
        .filter(|message| {
            message["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("textDocument/semanticTokens/"))
        })
        .collect())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_or_malformed_semantic_deltas_retry_a_full_snapshot() -> anyhow::Result<()> {
    for mode in ["--invalid-delta", "--malformed-delta"] {
        let mut f = Fixture::with_args(false, &["--deltas", mode]).await?;
        let response = f.response().await?;
        response(&mut f.app.editor);
        f.edit("😀 x\n");
        let response = f.response().await?;
        response(&mut f.app.editor);
        assert_eq!(
            f.app.editor.document(f.doc).unwrap().semantic_tokens()[0].range,
            2..3
        );
        let requests = semantic_requests(&f)?;
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[1]["method"],
            "textDocument/semanticTokens/full/delta"
        );
        assert_eq!(requests[2]["method"], "textDocument/semanticTokens/full");
        f.edit("😀 foo\n");
        let response = f.response().await?;
        response(&mut f.app.editor);
        let requests = semantic_requests(&f)?;
        assert_eq!(requests[3]["params"]["previousResultId"], "full-3");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_delta_responses_can_replace_or_clear_the_full_snapshot() -> anyhow::Result<()> {
    for mode in ["--delta-full", "--null-delta"] {
        let mut f = Fixture::with_args(false, &["--deltas", mode]).await?;
        let response = f.response().await?;
        response(&mut f.app.editor);
        f.edit("😀 x\n");
        let response = f.response().await?;
        response(&mut f.app.editor);
        let tokens = f.app.editor.document(f.doc).unwrap().semantic_tokens();
        if mode == "--null-delta" {
            assert!(tokens.is_empty());
        } else {
            assert_eq!(tokens[0].range, 2..3);
        }
        f.edit("😀 xyz\n");
        let response = f.response().await?;
        response(&mut f.app.editor);
        let requests = semantic_requests(&f)?;
        if mode == "--null-delta" {
            assert_eq!(requests[2]["method"], "textDocument/semanticTokens/full");
        } else {
            assert_eq!(requests[2]["params"]["previousResultId"], "delta-2");
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_or_unpublished_semantic_result_ids_cannot_be_delta_bases() -> anyhow::Result<()> {
    for missing_id in [false, true] {
        let args = if missing_id {
            vec!["--deltas", "--no-result-id"]
        } else {
            vec!["--deltas"]
        };
        let mut f = Fixture::with_args(false, &args).await?;
        let mut queued = Some(f.response().await?);
        if missing_id {
            queued.take().unwrap()(&mut f.app.editor);
        }
        f.edit("😀 x\n");
        if !missing_id {
            queued.take().unwrap()(&mut f.app.editor);
        }
        let response = f.response().await?;
        response(&mut f.app.editor);
        let requests = semantic_requests(&f)?;
        assert!(requests
            .iter()
            .all(|m| m["method"] == "textDocument/semanticTokens/full"));
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_semantic_deltas_cannot_replace_the_retained_base_after_an_edit(
) -> anyhow::Result<()> {
    let mut f = Fixture::with_args(false, &["--deltas"]).await?;
    let response = f.response().await?;
    response(&mut f.app.editor);
    f.edit("😀 x\n");
    let queued = f.response().await?;
    f.edit("😀 foo\n");
    queued(&mut f.app.editor);
    assert!(f
        .app
        .editor
        .document(f.doc)
        .unwrap()
        .semantic_tokens()
        .is_empty());
    let response = f.response().await?;
    response(&mut f.app.editor);
    assert_eq!(
        f.app.editor.document(f.doc).unwrap().semantic_tokens()[0].range,
        2..5
    );
    let requests = semantic_requests(&f)?;
    assert_eq!(requests[1]["params"]["previousResultId"], "full-1");
    assert_eq!(requests[2]["params"]["previousResultId"], "full-1");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_delta_bases_are_reset_when_the_feature_is_disabled_or_the_uri_changes(
) -> anyhow::Result<()> {
    for rename in [false, true] {
        let mut f = Fixture::with_args(false, &["--deltas"]).await?;
        let response = f.response().await?;
        response(&mut f.app.editor);
        f.edit("😀 x\n");
        let queued = f.response().await?;
        if rename {
            let path = f._dir.path().join("renamed.semantic-test");
            f.app.editor.set_doc_path(f.doc, &path);
        } else {
            let mut config = f.app.editor.config().clone();
            config.lsp.semantic_tokens.enable = false;
            f.app
                .apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(
                    config.clone(),
                )));
            queued(&mut f.app.editor);
            config.lsp.semantic_tokens.enable = true;
            f.app
                .apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(config)));
            let response = f.response().await?;
            response(&mut f.app.editor);
            assert_eq!(
                semantic_requests(&f)?.last().unwrap()["method"],
                "textDocument/semanticTokens/full"
            );
            assert!(f.app.close().await.is_empty());
            continue;
        }
        queued(&mut f.app.editor);
        let response = f.response().await?;
        response(&mut f.app.editor);
        let requests = semantic_requests(&f)?;
        assert_eq!(requests.len(), 3, "rename should request tokens once");
        assert_eq!(requests[2]["method"], "textDocument/semanticTokens/full");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_deltas_require_full_delta_support_even_when_results_have_ids(
) -> anyhow::Result<()> {
    for range_only in [false, true] {
        let mut f = Fixture::with_args(range_only, &["--deltas", "--no-delta-support"]).await?;
        let response = f.response().await?;
        response(&mut f.app.editor);
        f.edit("😀 x\n");
        let response = f.response().await?;
        response(&mut f.app.editor);
        let method = if range_only {
            "textDocument/semanticTokens/range"
        } else {
            "textDocument/semanticTokens/full"
        };
        let requests = semantic_requests(&f)?;
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|m| m["method"] == method));
        assert_eq!(
            f.app.editor.document(f.doc).unwrap().semantic_tokens()[0].range,
            2..3
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}
