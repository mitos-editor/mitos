use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use term::{application::Application, ui::lsp::signature_help::SignatureHelp};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    document::Mode,
    editor::{Action, ConfigEvent, EditorEvent},
    handlers::signature_help::{
        self, SignatureHelpChange, SignatureHelpHandler, SignatureHelpInvoked, SignatureHelpUpdate,
    },
};

use super::helpers::{run_event_loop_until_idle, test_config, test_syntax_loader, AppBuilder};

struct Fixture {
    app: Application,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    log: PathBuf,
    response_gate: PathBuf,
    initialize_gate: PathBuf,
    server_count: usize,
}

impl Fixture {
    async fn new(dir: &Path, text: &str, names: &[&str]) -> anyhow::Result<Self> {
        let mut fixture = Self::uninitialized(dir, text, names)?;
        fixture.initialize().await?;
        Ok(fixture)
    }

    fn uninitialized(dir: &Path, text: &str, names: &[&str]) -> anyhow::Result<Self> {
        let gate = dir.join("initialize-ready");
        if gate.exists() {
            std::fs::remove_file(&gate)?;
        }
        let response_gate = dir.join("respond-ready");
        std::fs::write(&response_gate, "ready")?;
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-test-lsp").into());
        let mut servers = String::new();
        for name in names {
            let log = dir.join(format!("{name}.jsonl"));
            let args = toml::Value::Array(
                [
                    "--signature-help",
                    name,
                    log.to_str().unwrap(),
                    "--initialize-gate",
                    gate.to_str().unwrap(),
                    "--response-gate",
                    response_gate.to_str().unwrap(),
                ]
                .into_iter()
                .map(|arg| toml::Value::String(arg.into()))
                .collect(),
            );
            servers.push_str(&format!(
                "[language-server.{name}]\ncommand = {command}\nargs = {args}\n"
            ));
        }
        let names_config = toml::Value::Array(
            names
                .iter()
                .map(|name| toml::Value::String((*name).into()))
                .collect(),
        );
        let loader = test_syntax_loader(Some(format!(
            r#"
            {servers}
            [[language]]
            name = "signature-test"
            scope = "source.signature-test"
            file-types = ["signature-test"]
            roots = []
            language-servers = {names_config}
        "#
        )));
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.lsp.auto_signature_help = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        let (tx, callbacks) = mpsc::unbounded_channel();
        let blocking = tx.clone();
        app.editor.handlers.signature_hints = SignatureHelpHandler::new(EditorCallbackSender::new(
            move |callback| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send((false, callback));
                }
            },
            move |callback| {
                let _ = blocking.send((true, callback));
            },
        ));
        let path = dir.join("document.signature-test");
        std::fs::write(&path, text)?;
        app.editor.open(&path, Action::Replace)?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
        Ok(Self {
            app,
            callbacks,
            log: dir.join("alpha.jsonl"),
            response_gate,
            initialize_gate: gate,
            server_count: names.len(),
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        std::fs::write(&self.initialize_gate, "ready")?;
        for _ in 0..self.server_count {
            let (server, call) = tokio::time::timeout(
                Duration::from_secs(10),
                self.app.editor.language_servers.incoming.next(),
            )
            .await?
            .context("server initialization")?;
            anyhow::ensure!(
                matches!(&call, lsp_client::Call::Notification(message) if message.method == "initialized")
            );
            self.app.handle_language_server_message(call, server).await;
        }
        Ok(())
    }

    fn trigger(&self, invoked: SignatureHelpInvoked) {
        self.app
            .editor
            .handlers
            .signature_hints
            .trigger(&self.app.editor, invoked);
    }

    async fn next(&mut self) -> anyhow::Result<(bool, EditorCallback)> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("signature callback queue closed")
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        loop {
            let (scheduling, callback) = self.next().await?;
            if scheduling {
                callback(&mut self.app.editor);
            } else {
                return Ok(callback);
            }
        }
    }

    async fn update(&mut self) -> anyhow::Result<SignatureHelpUpdate> {
        self.response().await?(&mut self.app.editor);
        signature_help::next_update(&self.app.editor).context("signature update missing")
    }

    fn take_change(&self) -> Option<SignatureHelpChange> {
        signature_help::next_update(&self.app.editor)
            .and_then(|update| update.resolve(&self.app.editor))
    }

    async fn label(&mut self) -> anyhow::Result<String> {
        match self.update().await?.resolve(&self.app.editor) {
            Some(SignatureHelpChange::Show(response)) => Ok(response.signatures[0].label.clone()),
            _ => anyhow::bail!("expected signatures"),
        }
    }

    fn select(&mut self, cursor: usize) {
        let (view, doc) = current!(self.app.editor);
        doc.set_selection(view.id, Selection::point(cursor));
    }

    fn edit(&mut self, text: &str) {
        let (view, doc) = current!(self.app.editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        assert!(doc.apply(&transaction, view.id));
    }

    fn automatic(&mut self, enabled: bool) {
        let mut config = (*self.app.editor.config()).clone();
        config.lsp.auto_signature_help = enabled;
        self.app
            .handle_config_events(ConfigEvent::Update(Box::new(config)));
    }

    async fn popup_selection(&mut self) -> anyhow::Result<Option<usize>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        term::job::dispatch(move |_, compositor| {
            let selection = SignatureHelp::visible_popup(compositor)
                .map(|popup| popup.contents().active_signature());
            let _ = tx.send(selection);
        })
        .await;
        self.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(5),
            run_event_loop_until_idle(&mut self.app),
        )
        .await?;
        Ok(rx.await?)
    }

    async fn key(&mut self, key: &str) -> anyhow::Result<()> {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(ui_core::input::parse_macro(key)?[0].into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(ui_core::input::parse_macro(key)?[0].into());
        self.app.handle_terminal_events(Ok(event)).await;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_and_automatic_requests_preserve_protocol_and_provider_selection(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 call(\n", &["alpha", "beta"]).await?;
    f.automatic(false);
    f.trigger(SignatureHelpInvoked::Manual);
    assert_eq!(f.label().await?, "alpha: 😀 call(");
    let request: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&f.log)?.lines().last().unwrap())?;
    assert_eq!(request["position"]["character"], 8);
    assert!(request.get("context").is_none());
    assert!(std::fs::read_to_string(dir.path().join("beta.jsonl"))?.is_empty());
    f.trigger(SignatureHelpInvoked::Automatic);
    assert!(
        tokio::time::timeout(Duration::from_millis(180), f.callbacks.recv())
            .await
            .is_err()
    );
    f.automatic(true);
    f.app.editor.mode = Mode::Insert;
    let triggered = tokio::time::Instant::now();
    signature_help::post_insert_char(&f.app.editor);
    let (scheduling, callback) = f.next().await?;
    assert!(scheduling);
    assert!(triggered.elapsed() >= Duration::from_millis(120));
    callback(&mut f.app.editor);
    assert_eq!(f.label().await?, "alpha: 😀 call(");
    f.select(3);
    assert_eq!(f.label().await?, "alpha: 😀 call(");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_responses_reject_edits_selection_renames_and_closed_documents() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 call(\n", &["alpha"]).await?;
    f.app.editor.mode = Mode::Insert;
    f.trigger(SignatureHelpInvoked::Manual);
    let old = f.response().await?;
    f.select(3);
    f.select(7);
    old(&mut f.app.editor);
    assert!(f.take_change().is_none());
    let old = f.response().await?;
    f.edit("😀 edited(\n");
    old(&mut f.app.editor);
    assert!(f.take_change().is_none());
    let old = f.response().await?;
    current!(f.app.editor)
        .1
        .set_path(Some(&dir.path().join("renamed.signature-test")));
    old(&mut f.app.editor);
    assert!(f.take_change().is_none());
    f.trigger(SignatureHelpInvoked::Manual);
    let old = f.response().await?;
    let id = current_ref!(f.app.editor).1.id();
    assert!(f.app.editor.close_document(id, true).is_ok());
    old(&mut f.app.editor);
    assert!(matches!(f.take_change(), Some(SignatureHelpChange::Hide)));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn newer_requests_mode_configuration_and_focus_invalidate_presentation() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 call(\n", &["alpha"]).await?;
    f.trigger(SignatureHelpInvoked::Manual);
    let old = f.response().await?;
    f.trigger(SignatureHelpInvoked::Manual);
    old(&mut f.app.editor);
    assert!(f.take_change().is_none());
    let update = f.update().await?;
    f.app.editor.mode = Mode::Normal;
    signature_help::mode_changed(&f.app.editor, Mode::Insert);
    f.app.editor.mode = Mode::Insert;
    assert!(update.resolve(&f.app.editor).is_none());
    f.trigger(SignatureHelpInvoked::Automatic);
    let update = f.update().await?;
    f.automatic(false);
    f.automatic(true);
    assert!(update.resolve(&f.app.editor).is_none());
    f.trigger(SignatureHelpInvoked::Manual);
    let update = f.update().await?;
    let id = current_ref!(f.app.editor).1.id();
    f.app.editor.new_file(Action::Replace);
    assert!(matches!(f.take_change(), Some(SignatureHelpChange::Hide)));
    f.app.editor.switch(id, Action::Replace);
    assert!(update.resolve(&f.app.editor).is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn simultaneous_editors_own_triggers_callbacks_and_updates() -> anyhow::Result<()> {
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let mut first = Fixture::new(a.path(), "😀 first(\n", &["alpha"]).await?;
    let mut second = Fixture::new(b.path(), "😀 second(\n", &["alpha"]).await?;
    assert_eq!(
        current_ref!(first.app.editor).1.id(),
        current_ref!(second.app.editor).1.id()
    );
    first.trigger(SignatureHelpInvoked::Manual);
    let wrong = first.response().await?;
    wrong(&mut second.app.editor);
    assert!(second.take_change().is_none());
    first.trigger(SignatureHelpInvoked::Manual);
    let update = first.update().await?;
    assert!(update.resolve(&second.app.editor).is_none());
    second.trigger(SignatureHelpInvoked::Manual);
    assert_eq!(second.label().await?, "alpha: 😀 second(");
    first.app.editor.mode = Mode::Insert;
    first.edit("😀 changed(\n");
    assert_eq!(first.label().await?, "alpha: 😀 changed(");
    assert!(second.callbacks.try_recv().is_err());
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_stops_inflight_work_and_later_requests_still_succeed() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 call(\n", &["alpha"]).await?;
    std::fs::remove_file(&f.response_gate)?;
    f.trigger(SignatureHelpInvoked::Manual);
    let (scheduling, callback) = f.next().await?;
    assert!(scheduling);
    callback(&mut f.app.editor);
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(&f.log)
            .unwrap_or_default()
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    f.app.editor.handlers.signature_hints.cancel();
    f.edit("😀 fresh(\n");
    f.trigger(SignatureHelpInvoked::Manual);
    std::fs::write(&f.response_gate, "ready")?;
    assert_eq!(f.label().await?, "alpha: 😀 fresh(");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn server_exit_replacement_and_drop_invalidate_work() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 call(\n", &["alpha"]).await?;
    f.trigger(SignatureHelpInvoked::Manual);
    let old = f.response().await?;
    let server = current_ref!(f.app.editor)
        .1
        .language_servers()
        .next()
        .unwrap()
        .id();
    f.app.editor.handle_language_server_exit(server);
    old(&mut f.app.editor);
    assert!(matches!(f.take_change(), Some(SignatureHelpChange::Hide)));
    assert!(f.app.close().await.is_empty());

    let mut f = Fixture::new(dir.path(), "😀 call(\n", &["alpha"]).await?;
    f.trigger(SignatureHelpInvoked::Manual);
    let old = f.response().await?;
    f.app.editor.handlers.signature_hints =
        SignatureHelpHandler::new(EditorCallbackSender::new(|_| async {}, |_| {}));
    old(&mut f.app.editor);
    assert!(f.take_change().is_none());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
            .await?
            .is_none()
    );
    assert!(f.app.close().await.is_empty());
    let f = Fixture::new(dir.path(), "😀 call(\n", &["alpha"]).await?;
    let Fixture {
        app, mut callbacks, ..
    } = f;
    drop(app);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), callbacks.recv())
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_forwards_triggers_and_keeps_popup_navigation_and_dismissal() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 call\n", &["alpha"]).await?;
    f.key("i").await?;
    f.key("(").await?;
    f.response().await?(&mut f.app.editor);
    // Other editor features can publish updates alongside signature help.
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = f.app.editor.wait_event().await;
            if matches!(event, EditorEvent::SignatureHelp(_)) {
                break event;
            }
            f.app.handle_editor_event(event).await;
        }
    })
    .await?;
    f.app.handle_editor_event(event).await;
    assert_eq!(f.popup_selection().await?, Some(0));
    f.key("<A-n>").await?;
    assert_eq!(f.popup_selection().await?, Some(1));
    f.key("<esc>").await?;
    assert_eq!(f.popup_selection().await?, None);
    for text in ["empty\n", "no-signatures\n"] {
        f.edit(text);
        f.trigger(SignatureHelpInvoked::Manual);
        let update = f.update().await?;
        assert!(matches!(
            update.resolve(&f.app.editor),
            Some(SignatureHelpChange::Hide)
        ));
    }
    f.app.editor.mode = Mode::Insert;
    f.edit("error\n");
    f.trigger(SignatureHelpInvoked::Manual);
    f.response().await?(&mut f.app.editor);
    assert!(f.take_change().is_none());
    // A transient error keeps retriggering available without a new trigger character.
    f.edit("recovered\n");
    assert_eq!(f.label().await?, "alpha: recovered");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_provider_is_reported_only_manually_and_retries_after_startup() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::uninitialized(dir.path(), "😀 call(\n", &["alpha"])?;
    f.app.editor.clear_status();
    f.app.editor.mode = Mode::Insert;
    f.trigger(SignatureHelpInvoked::Automatic);
    f.next().await?.1(&mut f.app.editor);
    assert!(f.app.editor.get_status().is_none());
    assert!(f.take_change().is_none());
    f.trigger(SignatureHelpInvoked::Manual);
    f.next().await?.1(&mut f.app.editor);
    assert_eq!(
        f.app.editor.get_status().unwrap().0.as_ref(),
        "No configured language server supports signature-help"
    );
    assert!(f.take_change().is_none());
    f.initialize().await?;
    f.edit("😀 ready(\n");
    assert_eq!(f.label().await?, "alpha: 😀 ready(");
    assert!(f.app.close().await.is_empty());
    Ok(())
}
