use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Context as _;
use editor_core::{diagnostic::DiagnosticProvider, Transaction, Uri};
use lsp_client::{lsp, Call, LanguageServerId, Notification};
use serde_json::{json, Value};
use term::application::Application;
use tokio_stream::StreamExt;
use view::{current, current_ref, editor::Action, events::LanguageServerExited};

use super::helpers::{test_config, test_syntax_loader, AppBuilder};

struct Fixture {
    app: Application,
    gate: PathBuf,
    logs: Vec<PathBuf>,
}

impl Fixture {
    fn new(dir: &Path, names: &[&str]) -> anyhow::Result<Self> {
        let gate = dir.join("initialize-ready");
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-test-lsp").into());
        let mut servers = String::new();
        let mut logs = Vec::new();
        for name in names {
            let log = dir.join(format!("{name}.jsonl"));
            let args = toml::Value::Array(
                [
                    "--lifecycle",
                    name,
                    log.to_str().unwrap(),
                    gate.to_str().unwrap(),
                ]
                .into_iter()
                .map(|s| toml::Value::String(s.into()))
                .collect(),
            );
            servers.push_str(&format!("[language-server.{name}]\ncommand = {command}\nargs = {args}\nconfig = {{ lifecycle = \"{name}\" }}\n"));
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
            name = "lifecycle-test"
            scope = "source.lifecycle-test"
            file-types = ["lifecycle-test"]
            roots = []
            language-servers = {names}
        "#
        )));
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.breadcrumb.enable = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        let path = dir.join("document.lifecycle-test");
        std::fs::write(&path, "😀 original\n")?;
        app.editor.open(&path, Action::Replace)?;
        Ok(Self { app, gate, logs })
    }

    fn release(&self) -> anyhow::Result<()> {
        std::fs::write(&self.gate, "ready")?;
        Ok(())
    }

    async fn next(&mut self) -> anyhow::Result<(LanguageServerId, Call)> {
        tokio::time::timeout(
            Duration::from_secs(10),
            self.app.editor.language_servers.incoming.next(),
        )
        .await?
        .context("LSP message stream closed")
    }

    /// Drive the shared editor API directly, without terminal message handling or rendering.
    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.release()?;
        let mut initialized = 0;
        while initialized < self.logs.len() {
            let (server_id, call) = self.next().await?;
            let Call::Notification(notification) = call else {
                anyhow::bail!("unexpected LSP request")
            };
            match Notification::parse(&notification.method, notification.params)? {
                Notification::Initialized => {
                    self.app
                        .editor
                        .handle_language_server_initialized(server_id);
                    initialized += 1;
                }
                Notification::PublishDiagnostics(params) => self
                    .app
                    .editor
                    .handle_publish_diagnostics(server_id, params),
                notification => anyhow::bail!("unexpected notification: {notification:?}"),
            }
        }
        Ok(())
    }

    fn server(&self, name: &str) -> LanguageServerId {
        self.app
            .editor
            .language_servers
            .iter_clients()
            .find(|server| server.name() == name)
            .unwrap()
            .id()
    }

    fn uri(&self) -> Uri {
        current_ref!(self.app.editor).1.uri().unwrap()
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
}

fn params(uri: &Uri, version: Option<i32>, message: &str) -> lsp::PublishDiagnosticsParams {
    serde_json::from_value(json!({
        "uri": uri.to_url().unwrap().to_string(), "version": version,
        "diagnostics": [{"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 6}}, "severity": 2, "message": message}]
    })).unwrap()
}

fn notification(method: &str, params: Value) -> Call {
    serde_json::from_value(json!({"jsonrpc": "2.0", "method": method, "params": params})).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn initialization_sends_configuration_before_open_and_feature_requests() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    f.initialize().await?;
    let (id, call) = f.next().await?;
    let Call::Notification(message) = call else {
        anyhow::bail!("expected diagnostic notification")
    };
    let Notification::PublishDiagnostics(params) =
        Notification::parse(&message.method, message.params)?
    else {
        anyhow::bail!("expected push diagnostics")
    };
    f.app.editor.handle_publish_diagnostics(id, params);
    assert_eq!(f.messages(), ["alpha"]);
    let diagnostic = &current_ref!(f.app.editor).1.diagnostics()[0];
    assert_eq!((diagnostic.range.start, diagnostic.range.end), (2, 5));
    let messages = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages: Vec<Value> = std::fs::read_to_string(&f.logs[0])?
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()?;
            if messages
                .iter()
                .any(|m| m["method"] == "textDocument/documentSymbol")
            {
                return anyhow::Ok(messages);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await??;
    let index = |method| {
        messages
            .iter()
            .position(|message| message["method"] == method)
            .unwrap()
    };
    let config = index("workspace/didChangeConfiguration");
    assert!(index("initialized") < config);
    assert!(config < index("textDocument/didOpen"));
    assert!(index("textDocument/didOpen") < index("textDocument/documentSymbol"));
    assert_eq!(
        messages[config]["params"]["settings"],
        json!({"lifecycle": "alpha"})
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pushes_reject_uninitialized_unknown_invalid_and_stale_updates() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    let id = f.server("alpha");
    let uri = f.uri();
    assert!(!f
        .app
        .editor
        .language_server_by_id(id)
        .unwrap()
        .is_initialized());
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, Some(0), "too early"));
    assert!(f.app.editor.diagnostics.is_empty());
    f.initialize().await?;
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, Some(0), "current"));
    assert_eq!(f.messages(), ["current"]);
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "x".into());
    assert!(doc.apply(&transaction, view.id));
    let version = doc.version();
    for stale in [version - 1, version + 1] {
        f.app
            .editor
            .handle_publish_diagnostics(id, params(&uri, Some(stale), "stale"));
        assert_eq!(f.messages(), ["current"]);
        assert_eq!(f.app.editor.diagnostics[&uri][0].0.message, "current");
    }
    let mut invalid = params(&uri, None, "invalid URI");
    invalid.uri = "https://example.test/diagnostic".parse()?;
    f.app.editor.handle_publish_diagnostics(id, invalid);
    assert_eq!(f.app.editor.diagnostics.len(), 1);
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, None, "unversioned"));
    assert_eq!(f.messages(), ["unversioned"]);
    let mut clear = params(&uri, Some(version), "unused");
    clear.diagnostics.clear();
    f.app.editor.handle_publish_diagnostics(id, clear);
    assert!(f.messages().is_empty());
    f.app.editor.handle_language_server_exit(id);
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, None, "after exit"));
    f.app.editor.handle_language_server_initialized(id);
    assert!(f.app.editor.diagnostics.is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_cleans_open_and_unopened_diagnostics_before_hooks_and_registry_removal(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha", "beta"])?;
    f.initialize().await?;
    let alpha = f.server("alpha");
    let beta = f.server("beta");
    let open = f.uri();
    // Match the normalization applied when diagnostic URLs become URI map keys.
    // Windows temporary directories can contain a verbatim path prefix.
    let shared = Uri::from(stdx::path::normalize(
        dir.path().join("unopened-shared.lifecycle-test"),
    ));
    let alpha_only = Uri::from(stdx::path::normalize(
        dir.path().join("unopened-alpha.lifecycle-test"),
    ));
    for uri in [&open, &shared] {
        f.app
            .editor
            .handle_publish_diagnostics(alpha, params(uri, None, "alpha"));
        f.app
            .editor
            .handle_publish_diagnostics(beta, params(uri, None, "beta"));
    }
    f.app
        .editor
        .handle_publish_diagnostics(alpha, params(&alpha_only, None, "alpha"));
    let pull = DiagnosticProvider::Lsp {
        server_id: alpha,
        identifier: Some("pull".into()),
    };
    f.app.editor.handle_lsp_diagnostics(
        &pull,
        open.clone(),
        None,
        params(&open, None, "alpha pull").diagnostics,
    );
    let doc = current!(f.app.editor).1;
    let mut spelling = doc.diagnostics()[0].clone();
    spelling.provider = DiagnosticProvider::Spelling;
    spelling.message = "spelling".into();
    doc.replace_diagnostics([spelling], &[], Some(&DiagnosticProvider::Spelling));
    let exits = Arc::new(AtomicUsize::new(0));
    let seen = exits.clone();
    event::register_hook!(move |event: &mut LanguageServerExited<'_>| {
        assert_eq!(event.server_id, alpha);
        assert!(event.editor.language_server_by_id(alpha).is_some());
        assert!(event
            .editor
            .documents()
            .any(|doc| doc.supports_language_server(alpha)));
        assert!(event
            .editor
            .diagnostics
            .values()
            .flatten()
            .all(|(_, provider)| provider.language_server_id() != Some(alpha)));
        assert!(event
            .editor
            .documents()
            .flat_map(|doc| doc.diagnostics())
            .all(|diagnostic| diagnostic.provider.language_server_id() != Some(alpha)));
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    f.app.editor.handle_language_server_exit(alpha);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(f.app.editor.language_server_by_id(alpha).is_none());
    assert!(f.app.editor.language_server_by_id(beta).is_some());
    assert_eq!(f.messages(), ["beta", "spelling"]);
    assert!(!f.app.editor.diagnostics.contains_key(&alpha_only));
    assert_eq!(f.app.editor.diagnostics[&shared].len(), 1);
    assert_eq!(f.app.editor.diagnostics[&shared][0].0.message, "beta");
    f.app.editor.handle_language_server_exit(alpha);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_adapter_delegates_and_keeps_the_exit_status_message() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    f.release()?;
    let (id, call) = f.next().await?;
    f.app.handle_language_server_message(call, id).await;
    let (id, call) = f.next().await?;
    f.app.handle_language_server_message(call, id).await;
    assert_eq!(f.messages(), ["alpha"]);
    event::register_hook!(move |event: &mut LanguageServerExited<'_>| {
        assert_eq!(
            event.editor.get_status().unwrap().0.as_ref(),
            "Language server exited: alpha"
        );
        Ok(())
    });
    f.app
        .handle_language_server_message(notification("exit", Value::Null), id)
        .await;
    assert!(f.app.editor.language_server_by_id(id).is_none());
    assert!(f.app.editor.diagnostics.is_empty());
    assert!(f.messages().is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}
