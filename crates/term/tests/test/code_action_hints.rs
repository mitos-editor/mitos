use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::{
    diagnostic::{Diagnostic, DiagnosticProvider},
    Selection, Transaction,
};
use lsp_client::lsp::{CodeActionKind, CodeActionTriggerKind};
use serde_json::Value;
use term::application::Application;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use view::{
    action::code_actions_for_range,
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    editor::{Action, ConfigEvent, StatusLineElement},
    handlers::code_action_hint::CodeActionHintHandler,
};

use super::helpers::{test_config, test_syntax_loader, AppBuilder};

struct Fixture {
    app: Application,
    callbacks: mpsc::Receiver<(bool, EditorCallback)>,
    log: PathBuf,
    gate: PathBuf,
    server_count: usize,
}

impl Fixture {
    fn new(dir: &Path, word: &str, servers: &[&str]) -> anyhow::Result<Self> {
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-test-lsp").into());
        let log = dir.join("alpha.jsonl");
        let gate = dir.join("initialize-ready");
        let server_config = servers
            .iter()
            .map(|name| {
                let args = toml::Value::Array(
                    [
                        "--code-actions".to_owned(),
                        (*name).into(),
                        dir.join(format!("{name}.jsonl")).to_str().unwrap().into(),
                        "--initialize-gate".into(),
                        gate.to_str().unwrap().into(),
                    ]
                    .into_iter()
                    .map(toml::Value::String)
                    .collect(),
                );
                format!("[language-server.{name}]\ncommand = {command}\nargs = {args}\n")
            })
            .collect::<String>();
        let names = toml::Value::Array(
            servers
                .iter()
                .map(|name| toml::Value::String((*name).into()))
                .collect(),
        );
        let loader = test_syntax_loader(Some(format!(
            r#"
            {server_config}
            [[language]]
            name = "code-action-test"
            scope = "source.code-action-test"
            file-types = ["action-test"]
            roots = []
            language-servers = {names}
        "#
        )));
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.statusline.right = vec![StatusLineElement::CodeActionHint];
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        let (tx, callbacks) = mpsc::channel(64);
        let blocking = tx.clone();
        app.editor.handlers.code_action_hint =
            CodeActionHintHandler::new(EditorCallbackSender::new(
                move |callback| {
                    let tx = tx.clone();
                    async move {
                        let _ = tx.send((false, callback)).await;
                    }
                },
                move |callback| event::send_blocking(&blocking, (true, callback)),
            ));
        let path = dir.join("document.action-test");
        std::fs::write(&path, format!("😀 {word}\n"))?;
        app.editor.open(&path, Action::Replace)?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::single(2, 5));
        Ok(Self {
            app,
            callbacks,
            log,
            gate,
            server_count: servers.len(),
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        std::fs::write(&self.gate, "ready")?;
        tokio::time::timeout(Duration::from_secs(10), async {
            // Transport readiness alone does not mean the editor has sent didOpen.
            // Process initialization for every server before requesting hints.
            for _ in 0..self.server_count {
                let (server_id, call) = self
                    .app
                    .editor
                    .language_servers
                    .incoming
                    .next()
                    .await
                    .context("LSP message stream closed")?;
                anyhow::ensure!(
                    matches!(&call, lsp_client::Call::Notification(message)
                    if message.method == "initialized"),
                    "expected initialization notification"
                );
                self.app
                    .handle_language_server_message(call, server_id)
                    .await;
            }
            anyhow::Ok(())
        })
        .await
        .context("code-action server initialization timed out")?
    }

    async fn next(&mut self) -> anyhow::Result<(bool, EditorCallback)> {
        tokio::time::timeout(Duration::from_secs(10), self.callbacks.recv())
            .await?
            .context("hint callback queue closed")
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

    async fn publish(&mut self) -> anyhow::Result<()> {
        self.response().await?(&mut self.app.editor);
        Ok(())
    }

    fn hint(&self) -> bool {
        let (view, doc) = current_ref!(self.app.editor);
        doc.code_action_hints(view.id)
    }

    fn edit(&mut self, word: &str) {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(2, doc.text().len_chars() - 1, Some(word.into()))].into_iter(),
        );
        assert!(doc.apply(&change, view.id));
    }

    fn selection(&mut self, start: usize, end: usize) {
        let (view, doc) = current!(self.app.editor);
        doc.set_selection(view.id, Selection::single(start, end));
    }

    fn diagnostics(&mut self, spelling: bool) {
        let (_, doc) = current!(self.app.editor);
        let doc_id = doc.id();
        let provider = DiagnosticProvider::Spelling;
        let diagnostics = spelling.then(|| Diagnostic {
            range: editor_core::diagnostic::Range { start: 2, end: 5 },
            starts_at_word: true,
            ends_at_word: true,
            zero_width: false,
            line: 0,
            message: "spelling".into(),
            severity: None,
            code: None,
            provider: provider.clone(),
            tags: Vec::new(),
            source: Some("spelling".into()),
            data: None,
        });
        doc.replace_diagnostics(diagnostics, &[], Some(&provider));
        event::dispatch(view::events::DiagnosticsDidChange {
            editor: &mut self.app.editor,
            doc: doc_id,
        });
    }

    fn enabled(&mut self, enabled: bool) {
        let mut config = (*self.app.editor.config()).clone();
        config.gutters.layout.clear();
        config.statusline.left.clear();
        config.statusline.right = if enabled {
            vec![StatusLineElement::CodeActionHint]
        } else {
            Vec::new()
        };
        self.app
            .handle_config_events(ConfigEvent::Update(Box::new(config)));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_preserve_utf16_diagnostics_triggers_and_kind_filters() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "enabled", &["alpha"])?;
    f.initialize().await?;
    f.diagnostics(true);
    f.publish().await?;
    assert!(f.hint());
    let requests: Vec<Value> = std::fs::read_to_string(&f.log)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let request = requests.last().unwrap();
    assert_eq!(request["range"]["start"]["character"], 3);
    assert_eq!(request["range"]["end"]["character"], 6);
    assert_eq!(request["context"]["triggerKind"], 2);
    assert_eq!(
        request["context"]["diagnostics"][0]["range"],
        request["range"]
    );
    assert!(request["context"].get("only").is_none());
    let (view, doc) = current_ref!(f.app.editor);
    let requests = code_actions_for_range(
        doc,
        doc.selection(view.id).primary(),
        Some(vec![CodeActionKind::SOURCE_ORGANIZE_IMPORTS]),
        CodeActionTriggerKind::INVOKED,
    );
    assert_eq!(requests.len(), 1);
    for (request, _) in requests {
        assert!(request.await?.is_some());
    }
    let requests: Vec<Value> = std::fs::read_to_string(&f.log)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let request = requests.last().unwrap();
    assert_eq!(request["context"]["triggerKind"], 1);
    assert_eq!(
        request["context"]["only"],
        serde_json::json!(["source.organizeImports"])
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_and_empty_actions_do_not_light_hints_but_commands_do() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "enabled", &["alpha"])?;
    f.initialize().await?;
    f.publish().await?;
    assert!(f.hint());
    for (text, available) in [("disabled", false), ("empty", false), ("command", true)] {
        f.edit(text);
        assert!(!f.hint());
        f.publish().await?;
        assert_eq!(f.hint(), available);
    }
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn simultaneous_editors_own_their_events_and_responses() -> anyhow::Result<()> {
    let first_dir = tempfile::tempdir()?;
    let second_dir = tempfile::tempdir()?;
    let mut first = Fixture::new(first_dir.path(), "enabled", &["alpha"])?;
    let mut second = Fixture::new(second_dir.path(), "disabled", &["alpha"])?;
    assert_eq!(
        current_ref!(first.app.editor).1.id(),
        current_ref!(second.app.editor).1.id()
    );
    for f in [&mut first, &mut second] {
        f.initialize().await?;
        f.publish().await?;
    }
    assert!(first.hint());
    assert!(!second.hint());
    first.edit("disabled");
    second.edit("enabled");
    first.publish().await?;
    second.publish().await?;
    assert!(!first.hint());
    assert!(second.hint());
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_results_reject_selection_edits_renames_and_document_closure() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "enabled", &["alpha"])?;
    f.initialize().await?;
    let old = f.response().await?;
    f.selection(3, 5);
    f.selection(2, 5);
    old(&mut f.app.editor);
    assert!(
        !f.hint(),
        "moving back to a selection must not revive canceled work"
    );
    let old = f.response().await?;
    f.edit("command");
    old(&mut f.app.editor);
    assert!(!f.hint());
    let old = f.response().await?;
    current!(f.app.editor)
        .1
        .set_path(Some(&dir.path().join("renamed.action-test")));
    old(&mut f.app.editor);
    assert!(!f.hint());
    f.selection(2, 3);
    let old = f.response().await?;
    let id = current_ref!(f.app.editor).1.id();
    assert!(f.app.editor.close_document(id, true).is_ok());
    old(&mut f.app.editor);
    assert!(f.app.editor.document(id).is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_results_follow_view_lifetime_and_its_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "enabled", &["alpha"])?;
    f.initialize().await?;
    let old = f.response().await?;
    let (view, doc) = current_ref!(f.app.editor);
    let (view_id, doc_id) = (view.id, doc.id());
    f.app.editor.new_file(Action::Replace);
    old(&mut f.app.editor);
    assert!(!f
        .app
        .editor
        .document(doc_id)
        .unwrap()
        .code_action_hints(view_id));
    f.app.editor.switch(doc_id, Action::Replace);
    f.selection(2, 3);
    let old = f.response().await?;
    f.app.editor.new_file(Action::HorizontalSplit);
    f.app.editor.close(view_id);
    old(&mut f.app.editor);
    assert!(!f
        .app
        .editor
        .document(doc_id)
        .unwrap()
        .code_action_hints(view_id));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn config_and_diagnostic_changes_invalidate_queued_results() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "disabled", &["alpha"])?;
    f.initialize().await?;
    f.diagnostics(true);
    f.publish().await?;
    assert!(f.hint());
    f.selection(3, 5);
    let old = f.response().await?;
    f.diagnostics(false);
    old(&mut f.app.editor);
    assert!(!f.hint());
    f.publish().await?;
    assert!(!f.hint());
    f.edit("enabled");
    let old = f.response().await?;
    f.enabled(false);
    f.enabled(true);
    old(&mut f.app.editor);
    assert!(!f.hint(), "re-enabling hints must not revive canceled work");
    f.publish().await?;
    assert!(f.hint());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_and_exited_servers_cannot_publish_queued_results() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "enabled", &["alpha", "beta"])?;
    f.initialize().await?;
    let old = f.response().await?;
    let server = current!(f.app.editor)
        .1
        .remove_language_server_by_name("alpha")
        .unwrap();
    old(&mut f.app.editor);
    assert!(!f.hint());
    f.selection(2, 3);
    f.publish().await?;
    assert!(f.hint());
    f.edit("command");
    let old = f.response().await?;
    let beta = current_ref!(f.app.editor)
        .1
        .language_servers()
        .next()
        .unwrap()
        .id();
    event::dispatch(view::events::LanguageServerExited {
        editor: &mut f.app.editor,
        server_id: beta,
    });
    f.app.editor.language_servers.remove_by_id(beta);
    old(&mut f.app.editor);
    assert!(!f.hint());
    // With no live LSP providers, the scheduled refresh can still show spelling actions.
    f.diagnostics(true);
    let (scheduling, callback) = f.next().await?;
    assert!(scheduling);
    callback(&mut f.app.editor);
    assert!(f.hint());
    drop(server);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn spelling_only_hints_follow_diagnostics_and_selection_without_lsp() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "misspelled", &[])?;
    f.diagnostics(true);
    for (selection, expected) in [(2, true), (7, false), (3, true)] {
        f.selection(selection, selection);
        let (scheduling, callback) = f.next().await?;
        assert!(scheduling);
        callback(&mut f.app.editor);
        assert_eq!(f.hint(), expected);
    }
    f.diagnostics(false);
    assert!(!f.hint());
    let (scheduling, callback) = f.next().await?;
    assert!(scheduling);
    callback(&mut f.app.editor);
    assert!(!f.hint());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn server_exit_refreshes_hints_from_remaining_providers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "enabled", &["alpha", "beta"])?;
    f.initialize().await?;
    let old = f.response().await?;
    let alpha = current_ref!(f.app.editor)
        .1
        .language_servers()
        .find(|server| server.name() == "alpha")
        .unwrap()
        .id();
    let count = std::fs::read_to_string(&f.log)?.lines().count();
    event::dispatch(view::events::LanguageServerExited {
        editor: &mut f.app.editor,
        server_id: alpha,
    });
    f.app.editor.language_servers.remove_by_id(alpha);
    old(&mut f.app.editor);
    assert!(!f.hint());
    f.publish().await?;
    assert!(f.hint());
    assert_eq!(std::fs::read_to_string(&f.log)?.lines().count(), count);
    assert!(f.app.close().await.is_empty());
    Ok(())
}
