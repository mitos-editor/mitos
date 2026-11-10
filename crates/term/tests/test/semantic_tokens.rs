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
        let dir = tempfile::tempdir()?;
        let (mut app, callbacks, gate) = lsp::feature_app(
            dir.path(),
            "😀 foo\n",
            "semantic-test",
            &["semantic"],
            test_config(),
            |name| {
                let server = ServerConfig::feature(name, "--semantic-tokens", dir.path());
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
    for range_only in [false, true] {
        let mut f = Fixture::new(range_only).await?;
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
            true
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
    assert!(f.app.close().await.is_empty());
    Ok(())
}
