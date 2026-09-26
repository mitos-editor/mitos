use std::{path::Path, time::Duration};

use anyhow::Context as _;
use editor_core::{diagnostic::DiagnosticProvider, Transaction};
use lsp_client::LanguageServerId;
use serde_json::Value;
use term::application::Application;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    editor::Action,
    handlers::diagnostics::pull::{self, PullDiagnosticsHandler},
    DocumentId,
};

use super::helpers::{test_config, test_syntax_loader, AppBuilder};

struct Fixture {
    app: Application,
    callbacks: mpsc::Receiver<(bool, EditorCallback)>,
    logs: Vec<std::path::PathBuf>,
    gate: std::path::PathBuf,
}

impl Fixture {
    fn new(directory: &Path, word: &str, names: &[&str], inter_file: bool) -> anyhow::Result<Self> {
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-test-lsp").into());
        let gate = directory.join("initialize-ready");
        let mut servers = String::new();
        let mut logs = Vec::new();
        for name in names {
            let log = directory.join(format!("{name}.jsonl"));
            let mut args = vec![
                "--diagnostics".into(),
                (*name).to_owned(),
                log.to_str().unwrap().to_owned(),
                "--initialize-gate".into(),
                gate.to_str().unwrap().to_owned(),
            ];
            if inter_file {
                args.push("--inter-file".into());
            }
            let args = toml::Value::Array(args.into_iter().map(toml::Value::String).collect());
            servers.push_str(&format!(
                "[language-server.{name}]\ncommand = {command}\nargs = {args}\n"
            ));
            logs.push(log);
        }
        let names = toml::Value::Array(
            names
                .iter()
                .map(|name| toml::Value::String((*name).into()))
                .collect(),
        );
        let loader = test_syntax_loader(Some(format!(
            r#"
            {servers}
            [[language]]
            name = "pull-diagnostic-test"
            scope = "source.pull-diagnostic-test"
            file-types = ["pull-test"]
            roots = []
            language-servers = {names}
        "#
        )));
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        let (tx, callbacks) = mpsc::channel(64);
        let blocking = tx.clone();
        app.editor.handlers.pull_diagnostics =
            PullDiagnosticsHandler::new(EditorCallbackSender::new(
                move |callback| {
                    let tx = tx.clone();
                    async move {
                        let _ = tx.send((false, callback)).await;
                    }
                },
                move |callback| event::send_blocking(&blocking, (true, callback)),
            ));
        let mut fixture = Self {
            app,
            callbacks,
            logs,
            gate,
        };
        fixture.open(&directory.join("document.pull-test"), word)?;
        Ok(fixture)
    }

    fn open(&mut self, path: &Path, word: &str) -> anyhow::Result<DocumentId> {
        std::fs::write(path, format!("😀 {word}\n"))?;
        Ok(self.app.editor.open(path, Action::Replace)?)
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        std::fs::write(&self.gate, "ready")?;
        tokio::time::timeout(Duration::from_secs(10), async {
            // Client capabilities become available before the editor processes
            // initialization. Handle every notification before consuming responses.
            for _ in 0..self.logs.len() {
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
        .context("pull diagnostic initialization did not complete")?
    }

    /// Debounce callbacks start work. Keep publication/retry callbacks under the
    /// test's control so state can change after the server has already replied.
    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (scheduling, callback) =
                    self.callbacks.recv().await.expect("callback queue closed");
                if scheduling {
                    callback(&mut self.app.editor);
                } else {
                    return callback;
                }
            }
        })
        .await
        .context("expected a queued diagnostic response or retry")
    }

    async fn publish(&mut self) -> anyhow::Result<()> {
        let callback = self.response().await?;
        callback(&mut self.app.editor);
        Ok(())
    }

    fn request(&mut self) {
        let id = current_ref!(self.app.editor).1.id();
        pull::request_document_diagnostics(&mut self.app.editor, id);
    }

    fn edit(&mut self, word: &str) {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(2, doc.text().len_chars() - 1, Some(word.into()))].into_iter(),
        );
        assert!(doc.apply(&change, view.id));
    }

    fn server(&self, name: &str) -> LanguageServerId {
        current_ref!(self.app.editor)
            .1
            .language_servers()
            .find(|server| server.name() == name)
            .unwrap()
            .id()
    }

    fn messages(&self) -> Vec<String> {
        let mut messages: Vec<_> = current_ref!(self.app.editor)
            .1
            .diagnostics()
            .iter()
            .map(|d| d.message.to_string())
            .collect();
        messages.sort();
        messages
    }

    fn requests(&self, server: usize) -> anyhow::Result<Vec<Value>> {
        std::fs::read_to_string(&self.logs[server])?
            .lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn full_and_unchanged_reports_preserve_provider_ids_and_utf16_ranges() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original"]);
    let diagnostic = &current_ref!(f.app.editor).1.diagnostics()[0];
    assert_eq!((diagnostic.range.start, diagnostic.range.end), (2, 10));
    assert!(
        matches!(&diagnostic.provider, DiagnosticProvider::Lsp { identifier: Some(id), .. } if id.as_ref() == "alpha")
    );
    f.request();
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original"]);
    let requests = f.requests(0)?;
    assert!(requests[0].get("previousResultId").is_none());
    assert_eq!(requests[1]["previousResultId"], "alpha:0");
    assert_eq!(requests[1]["identifier"], "alpha");

    f.edit("no-id");
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 no-id"]);
    f.request();
    f.publish().await?;
    assert!(f
        .requests(0)?
        .last()
        .unwrap()
        .get("previousResultId")
        .is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_refresh_does_not_cancel_other_providers_or_overwrite_their_results(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha", "beta"], false)?;
    f.initialize().await?;
    let first = f.response().await?;
    let second = f.response().await?;
    let alpha = f.server("alpha");
    pull::request_all_document_diagnostics_for_language_server(&mut f.app.editor, alpha);
    first(&mut f.app.editor);
    second(&mut f.app.editor);
    assert_eq!(f.messages(), ["beta: 😀 original"]);
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original", "beta: 😀 original"]);
    assert_eq!(f.requests(1)?.len(), 1);
    assert!(
        f.requests(0)?[1].get("previousResultId").is_none(),
        "discarded responses must not advance result IDs"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn events_and_responses_stay_with_simultaneous_editors() -> anyhow::Result<()> {
    let first_dir = tempfile::tempdir()?;
    let second_dir = tempfile::tempdir()?;
    let mut first = Fixture::new(first_dir.path(), "first", &["alpha"], false)?;
    let mut second = Fixture::new(second_dir.path(), "second", &["alpha"], false)?;
    assert_eq!(
        current_ref!(first.app.editor).1.id(),
        current_ref!(second.app.editor).1.id()
    );
    for f in [&mut first, &mut second] {
        f.initialize().await?;
        f.publish().await?;
    }
    first.edit("updated");
    second.edit("another");
    first.publish().await?;
    second.publish().await?;
    assert_eq!(first.messages(), ["alpha: 😀 updated"]);
    assert_eq!(second.messages(), ["alpha: 😀 another"]);
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_reports_cannot_publish_after_edits_renames_or_closure() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    let old = f.response().await?;
    f.edit("updated");
    old(&mut f.app.editor);
    assert!(f.messages().is_empty());
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 updated"]);

    f.edit("renamed");
    let old = f.response().await?;
    let old_uri = current_ref!(f.app.editor).1.uri().unwrap();
    current!(f.app.editor)
        .1
        .set_path(Some(&dir.path().join("renamed.pull-test")));
    old(&mut f.app.editor);
    // Existing diagnostics may remain mapped through the edit, but the queued
    // report must change neither the renamed buffer nor the old URI's cache.
    assert_eq!(f.messages(), ["alpha: 😀 updated"]);
    assert_eq!(
        f.app.editor.diagnostics[&old_uri][0].0.message,
        "alpha: 😀 updated"
    );
    // Closing before publication must not insert an entry into the global cache.
    let id = f.open(&dir.path().join("closed.pull-test"), "closed")?;
    let old = f.response().await?;
    let uri = current_ref!(f.app.editor).1.uri().unwrap();
    assert!(f.app.editor.close_document(id, true).is_ok());
    old(&mut f.app.editor);
    assert!(!f.app.editor.diagnostics.contains_key(&uri));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn server_retry_uses_the_owner_and_a_superseded_retry_cannot_restart_work(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "retry", &["alpha"], false)?;
    f.initialize().await?;
    f.publish().await?; // apply the server-requested retry
    f.publish().await?; // apply its successful report
    assert_eq!(f.messages(), ["alpha: 😀 retry"]);
    assert_eq!(f.requests(0)?.len(), 2);

    f.edit("retry-again");
    let retry = f.response().await?;
    f.request(); // supersede the retry while it waits in the callback queue
    retry(&mut f.app.editor);
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 retry-again"]);
    assert_eq!(f.requests(0)?.len(), 4);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inter_file_dependencies_refresh_other_open_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "first", &["alpha"], true)?;
    f.initialize().await?;
    f.publish().await?;
    let first_uri = current_ref!(f.app.editor).1.url().unwrap().to_string();
    f.open(&dir.path().join("second.pull-test"), "second")?;
    f.publish().await?;
    f.edit("updated");
    f.publish().await?; // the document's 250 ms debounce
    f.publish().await?; // inter-file refresh, first document
    f.publish().await?; // inter-file refresh, second document (order is irrelevant)
    assert_eq!(f.messages(), ["alpha: 😀 updated"]);
    assert_eq!(
        f.requests(0)?
            .iter()
            .filter(|r| r["textDocument"]["uri"] == first_uri)
            .count(),
        2
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn exited_servers_cannot_publish_queued_reports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    let response = f.response().await?;
    let server_id = f.server("alpha");
    event::dispatch(view::events::LanguageServerExited {
        editor: &mut f.app.editor,
        server_id,
    });
    f.app.editor.language_servers.remove_by_id(server_id);
    response(&mut f.app.editor);
    assert!(f.messages().is_empty());
    assert!(f.app.editor.diagnostics.is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_refresh_request_targets_only_the_requesting_provider() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha", "beta"], false)?;
    f.initialize().await?;
    f.publish().await?;
    f.publish().await?;
    let alpha = f.server("alpha");
    let call = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": "refresh", "method": "workspace/diagnostic/refresh"
    }))?;
    f.app.handle_language_server_message(call, alpha).await;
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original", "beta: 😀 original"]);
    assert_eq!(f.requests(0)?.len(), 2);
    assert_eq!(f.requests(1)?.len(), 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}
