//! Rename preparation and application run through shared editor operations.
use std::{path::Path, time::Duration};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use term::application::TestApplication;
use tokio::sync::mpsc;
use view::{callbacks::EditorCallback, handlers::Handlers};
use view::{current, current_ref, editor::Action, features::rename::RenameSession};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

struct Fixture {
    app: TestApplication,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
}

impl Fixture {
    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (blocking, callback) = self
                    .callbacks
                    .recv()
                    .await
                    .context("callback queue closed")?;
                if !blocking {
                    return Ok(callback);
                }
                callback(&mut self.app.editor);
            }
        })
        .await?
    }
}

async fn fixture(dir: &Path, args: &[&str]) -> anyhow::Result<Fixture> {
    let initialize = Gate::new(dir.join("initialize"));
    let mut server = ServerConfig::feature("rename", "--rename", dir).initialize_gate(&initialize);
    for arg in args {
        server = server.arg(arg);
    }
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let (sender, callbacks) =
        super::helpers::callbacks::unbounded(|blocking, callback| (blocking, callback));
    let (background, _) = super::helpers::callbacks::channel();
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_handler_setup(move |handlers, config| {
            *handlers = Handlers::new(config, sender);
            handlers.syntax = view::features::syntax::SyntaxHandler::new(background.clone());
            handlers.document_links =
                view::features::document_links::DocumentLinksHandler::new(background);
        })
        .with_lang_loader(lsp::syntax_loader(
            "rename",
            "rename",
            &["rename"],
            &server.toml(),
        ))
        .build()?;
    let path = dir.join("document.rename");
    std::fs::write(&path, "😀 foo\n")?;
    app.editor.open(&path, Action::Replace)?;
    initialize.release()?;
    lsp::initialize(&mut app, 1).await?;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::point(3));
    Ok(Fixture { app, callbacks })
}

async fn prepare(app: &mut TestApplication) -> anyhow::Result<RenameSession> {
    let (view, doc) = current_ref!(app.editor);
    let task = app.editor.prepare_rename(doc.id(), view.id)?;
    tokio::time::timeout(Duration::from_secs(5), task.run())
        .await?
        .context("preparation canceled")?
        .into_result(&app.editor)
        .context("preparation invalid")?
}

async fn wait_message(dir: &Path, method: &str) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(message) = lsp::read_log(&lsp::log_path(dir, "rename"))?
                .into_iter()
                .find(|message| message["method"] == method)
            {
                return Ok(message);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_activity_covers_requests_but_not_the_name_prompt() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = fixture(dir.path(), &[]).await?;
    let (view, doc) = current_ref!(f.app.editor);
    let document = doc.id();
    let task = f.app.editor.prepare_rename(document, view.id)?;
    let activity = f
        .app
        .editor
        .activities()
        .next()
        .expect("prepare-rename activity");
    assert_eq!(activity.label, "Renaming");
    assert_eq!(activity.document, Some(document));
    let update = task.run().await.context("preparation canceled")?;
    assert_eq!(f.app.editor.activities().count(), 1);
    let session = update
        .into_result(&f.app.editor)
        .context("preparation invalid")??;
    assert!(
        f.app.editor.activities().next().is_none(),
        "entering the new name is idle"
    );
    f.app.editor.rename_symbol(session, "renamed".into())?;
    assert_eq!(f.app.editor.activities().count(), 1);
    let callback = f.response().await?;
    assert_eq!(
        f.app.editor.activities().count(),
        1,
        "queued edits are still pending"
    );
    callback(&mut f.app.editor);
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "😀 renamed\n"
    );
    assert!(f.app.editor.activities().next().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_activity_clears_on_dropped_preparation_and_canceled_requests() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let response = Gate::new(dir.path().join("rename-response"));
    response.close()?;
    let mut f = fixture(
        dir.path(),
        &["--response-gate", response.path().to_str().unwrap()],
    )
    .await?;
    let (view, doc) = current_ref!(f.app.editor);
    let task = f.app.editor.prepare_rename(doc.id(), view.id)?;
    assert_eq!(f.app.editor.activities().count(), 1);
    drop(task);
    assert!(f.app.editor.activities().next().is_none());

    let session = prepare(&mut f.app).await?;
    assert!(f.app.editor.activities().next().is_none());
    f.app.editor.rename_symbol(session, "renamed".into())?;
    wait_message(dir.path(), "textDocument/rename").await?;
    assert_eq!(f.app.editor.activities().count(), 1);
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(0));
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.app.editor.activities().next().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    response.release()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_uses_the_prepared_document_and_utf16_position_after_focus_moves(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = fixture(dir.path(), &[]).await?;
    let app = &mut f.app;
    let document = current_ref!(app.editor).1.id();
    let session = prepare(app).await?;
    assert_eq!(session.prefill(), "foo");
    let other = app.editor.new_file(Action::Replace);
    let other_text = app.editor.document(other).unwrap().text().to_string();
    app.editor.rename_symbol(session, "renamed".into())?;
    let request = wait_message(dir.path(), "textDocument/rename").await?;
    assert_eq!(request["params"]["position"]["character"], 4);
    f.response().await?(&mut f.app.editor);
    assert_eq!(
        f.app.editor.document(document).unwrap().text().to_string(),
        "😀 renamed\n"
    );
    assert_eq!(current_ref!(f.app.editor).1.id(), other);
    assert_eq!(
        f.app.editor.document(other).unwrap().text().to_string(),
        other_text
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_preparation_rejects_invalid_ranges_and_superseded_targets() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = fixture(dir.path(), &["--invalid-range"]).await?;
    let app = &mut f.app;
    assert!(prepare(app).await.is_err());
    assert!(app.editor.activities().next().is_none());
    let (view, doc) = current_ref!(app.editor);
    let task = app.editor.prepare_rename(doc.id(), view.id)?;
    let update = task.run().await.context("missing preparation")?;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::point(0));
    assert!(update.into_result(&app.editor).is_none());
    assert!(app.editor.activities().next().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_without_prepare_support_uses_the_captured_word_and_rejects_edits(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = fixture(dir.path(), &["--no-prepare"]).await?;
    let app = &mut f.app;
    let session = prepare(app).await?;
    assert_eq!(session.prefill(), "foo");
    let (view, doc) = current!(app.editor);
    let change = Transaction::change(doc.text(), [(2, 5, Some("new".into()))].into_iter());
    view::editing::apply(doc, &change, view.id).unwrap();
    assert!(app
        .editor
        .rename_symbol(session, "unexpected".into())
        .is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_rename_reply_cannot_overwrite_a_newer_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = fixture(dir.path(), &[]).await?;
    let session = prepare(&mut f.app).await?;
    f.app.editor.rename_symbol(session, "renamed".into())?;
    let callback = f.response().await?;
    assert_eq!(f.app.editor.activities().count(), 1);
    let (view, doc) = current!(f.app.editor);
    let change = Transaction::change(doc.text(), [(2, 5, Some("new".into()))].into_iter());
    view::editing::apply(doc, &change, view.id).unwrap();
    callback(&mut f.app.editor);
    assert!(f.app.editor.activities().next().is_none());
    assert_eq!(current_ref!(f.app.editor).1.text().to_string(), "😀 new\n");
    Ok(())
}
