//! Request lifetime and presentation contracts through the shared editor API.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use term::application::Application;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    editor::Action,
    handlers::{
        hover::{self, HoverHandler, HoverUpdate},
        inlay_hints::{self, InlayHintsHandler},
    },
};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

struct Fixture {
    app: Application,
    callbacks: mpsc::UnboundedReceiver<EditorCallback>,
    log: PathBuf,
}

impl Fixture {
    async fn new(dir: &Path) -> anyhow::Result<Self> {
        Self::with_args(dir, &[]).await
    }

    async fn with_args(dir: &Path, args: &[&str]) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize-ready"));
        let mut server =
            ServerConfig::feature("alpha", "--language-features", dir).initialize_gate(&gate);
        for arg in args {
            server = server.arg(arg);
        }
        let servers = server.toml();
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.lsp.inlay_hints.enable = true;
        config.editor.lsp.inlay_hints.length_limit = Some(3.try_into().unwrap());
        let (sender, callbacks) = super::helpers::callbacks::unbounded(|_, callback| callback);
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(lsp::syntax_loader(
                "hover-test",
                "hover-test",
                &["alpha"],
                &servers,
            ))
            .with_handler_setup(move |handlers, _| {
                handlers.hover = HoverHandler::new(sender.clone());
                handlers.inlay_hints = InlayHintsHandler::new(sender);
            })
            .build()?;
        let path = dir.join("document.hover-test");
        std::fs::write(&path, "😀 original\n")?;
        app.editor.open(&path, Action::Replace)?;
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        Ok(Self {
            app,
            callbacks,
            log: lsp::log_path(dir, "alpha"),
        })
    }

    async fn wait_message(&self, method: &str) -> anyhow::Result<serde_json::Value> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(message) = std::fs::read_to_string(&self.log)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .find(|message| message["method"] == method)
                {
                    return message;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .with_context(|| format!("server did not receive {method}"))
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("feature callback queue closed")
    }

    async fn hover(&mut self) -> anyhow::Result<HoverUpdate> {
        hover::request(&mut self.app.editor);
        self.response().await?(&mut self.app.editor);
        hover::next_update(&self.app.editor).context("hover update missing")
    }

    fn edit(&mut self) {
        let (view, doc) = current!(self.app.editor);
        let transaction = Transaction::insert(doc.text(), &Selection::point(0), "new ".into());
        doc.apply(&transaction, view.id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_rejects_edits_before_publication_and_presentation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    hover::request(&mut f.app.editor);
    let response = f.response().await?;
    f.edit();
    response(&mut f.app.editor);
    assert!(hover::next_update(&f.app.editor).is_none());
    let update = f.hover().await?;
    f.edit();
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_replacement_and_selection_changes_invalidate_queued_updates() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let old = f.hover().await?;
    let new = f.hover().await?;
    assert!(old.resolve(&f.app.editor).is_none());
    assert!(new.resolve(&f.app.editor).is_some());
    let update = f.hover().await?;
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(2));
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_rejects_view_switches_and_detached_servers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.hover().await?;
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.new_file(Action::HorizontalSplit);
    assert!(update.resolve(&f.app.editor).is_none());
    f.app.editor.switch(document, Action::Replace);
    let update = f.hover().await?;
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("alpha");
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_reject_edits_and_reuse_pending_and_completed_requests() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    inlay_hints::refresh(&mut f.app.editor);
    let response = f.response().await?;
    inlay_hints::refresh(&mut f.app.editor);
    assert!(f.callbacks.try_recv().is_err());
    f.edit();
    response(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_none());
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    let hints = doc.inlay_hints(view.id).unwrap();
    assert_eq!(hints.type_inlay_hints[0].char_idx, 3);
    assert_eq!(hints.type_inlay_hints[0].text, "界é…");
    inlay_hints::refresh(&mut f.app.editor);
    assert!(f.callbacks.try_recv().is_err());
    // The hover response is a transport barrier: prior inlay requests have reached
    // the server, so the log can prove both pending and completed requests were reused.
    f.hover().await?;
    let requests = std::fs::read_to_string(&f.log)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "textDocument/inlayHint")
            .count(),
        2
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_reject_view_reuse_and_server_detachment() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    inlay_hints::refresh(&mut f.app.editor);
    let response = f.response().await?;
    let (view, doc) = current_ref!(f.app.editor);
    let view_id = view.id;
    let document = doc.id();
    f.app.editor.new_file(Action::Replace);
    response(&mut f.app.editor);
    assert!(f
        .app
        .editor
        .document(document)
        .unwrap()
        .inlay_hints(view_id)
        .is_none());
    f.app.editor.switch(document, Action::Replace);
    inlay_hints::refresh(&mut f.app.editor);
    let response = f.response().await?;
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("alpha");
    response(&mut f.app.editor);
    assert!(current_ref!(f.app.editor).1.inlay_hints(view_id).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_refresh_each_split_after_an_edit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let mut config = (*f.app.editor.config()).clone();
    config.lsp.inlay_hints.length_limit = None;
    f.app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.switch(document, Action::HorizontalSplit);
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    f.edit();
    inlay_hints::refresh(&mut f.app.editor);
    // Publishing one split's result must not mark the other split current.
    f.response().await?(&mut f.app.editor);
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let views: Vec<_> = f.app.editor.tree.views().map(|(view, _)| view.id).collect();
    assert_eq!(views.len(), 2);
    let doc = f.app.editor.document(document).unwrap();
    for view in views {
        assert!(doc.inlay_hints(view).unwrap().type_inlay_hints[0]
            .text
            .contains("new "));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_rejects_renamed_and_closed_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.hover().await?;
    let document = current_ref!(f.app.editor).1.id();
    current!(f.app.editor)
        .1
        .set_path(Some(&dir.path().join("renamed.hover-test")));
    assert!(update.resolve(&f.app.editor).is_none());
    let update = f.hover().await?;
    f.app.editor.new_file(Action::Replace);
    assert!(f.app.editor.close_document(document, true).is_ok());
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_reject_superseded_viewports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    {
        let (view, doc) = current!(f.app.editor);
        view.area.height = 10;
        let text = "😀 line\n".repeat(200);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        assert!(doc.apply(&transaction, view.id));
    }
    inlay_hints::refresh(&mut f.app.editor);
    let old = f.response().await?;
    let (view, doc) = current!(f.app.editor);
    doc.set_view_offset(
        view.id,
        view::view::ViewPosition {
            anchor: doc.text().line_to_char(100),
            ..Default::default()
        },
    );
    inlay_hints::refresh(&mut f.app.editor);
    old(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_none());
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).unwrap().id.first_line > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_buffers_cancels_inflight_hover_requests() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--hold-hover"]).await?;
    hover::request(&mut f.app.editor);
    let request = f.wait_message("textDocument/hover").await?;
    f.app.editor.new_file(Action::Replace);
    let cancel = f.wait_message("$/cancelRequest").await?;
    assert_eq!(cancel["params"]["id"], request["id"]);
    assert!(hover::next_update(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_reload_invalidates_hover_and_inlay_hints() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_some());
    hover::request(&mut f.app.editor);
    let callback = f.response().await?;
    disable_language_features(&mut f.app.editor)?;
    callback(&mut f.app.editor);
    assert!(hover::next_update(&f.app.editor).is_none());
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_none());
    Ok(())
}

fn disable_language_features(editor: &mut view::Editor) -> anyhow::Result<()> {
    let loader = super::helpers::test_syntax_loader(Some(
        r#"
        [[language]]
        name = "hover-test"
        scope = "source.hover-test"
        file-types = ["hover-test"]
        roots = []
        language-servers = [{ name = "alpha", except-features = ["hover", "inlay-hints"] }]
    "#
        .into(),
    ));
    editor.apply_language_config(loader, editor.theme.clone())?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_reload_rejects_hover_awaiting_presentation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.hover().await?;
    disable_language_features(&mut f.app.editor)?;
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_publish_for_unfocused_views_after_selection_changes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let (view, doc) = current_ref!(f.app.editor);
    let background = view.id;
    let document = doc.id();
    f.app.editor.switch(document, Action::HorizontalSplit);
    let focused = f.app.editor.tree.focus;
    assert_ne!(focused, background);
    inlay_hints::refresh(&mut f.app.editor);
    let first = f.response().await?;
    let second = f.response().await?;
    f.app
        .editor
        .document_mut(document)
        .unwrap()
        .set_selection(background, Selection::point(4));
    first(&mut f.app.editor);
    second(&mut f.app.editor);
    assert_eq!(f.app.editor.tree.focus, focused);
    let doc = f.app.editor.document(document).unwrap();
    assert!(doc.inlay_hints(background).is_some());
    assert!(doc.inlay_hints(focused).is_some());
    Ok(())
}
