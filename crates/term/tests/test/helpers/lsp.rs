//! Shared stdio fixture for server lifecycle and workspace requests.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::Uri;
use lsp_client::{Call, LanguageServerId, Notification};
use term::application::Application;
use tokio_stream::StreamExt;
use view::{current_ref, editor::Action};

use super::{test_config, test_syntax_loader, AppBuilder};

pub(crate) struct Fixture {
    pub app: Application,
    gate: PathBuf,
    pub logs: Vec<PathBuf>,
}

impl Fixture {
    pub fn new(dir: &Path, names: &[&str]) -> anyhow::Result<Self> {
        Self::with_config(dir, names, |name| {
            Some(format!("{{ lifecycle = \"{name}\" }}"))
        })
    }

    pub fn with_config(
        dir: &Path,
        names: &[&str],
        config: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
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
            servers.push_str(&format!(
                "[language-server.{name}]\ncommand = {command}\nargs = {args}\n"
            ));
            if let Some(config) = config(name) {
                servers.push_str(&format!("config = {config}\n"));
            }
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

    pub fn release(&self) -> anyhow::Result<()> {
        std::fs::write(&self.gate, "ready")?;
        Ok(())
    }

    pub async fn next(&mut self) -> anyhow::Result<(LanguageServerId, Call)> {
        tokio::time::timeout(
            Duration::from_secs(10),
            self.app.editor.language_servers.incoming.next(),
        )
        .await?
        .context("LSP message stream closed")
    }

    /// Drive the shared editor API directly, without terminal message handling or rendering.
    pub async fn initialize(&mut self) -> anyhow::Result<()> {
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

    pub fn server(&self, name: &str) -> LanguageServerId {
        self.app
            .editor
            .language_servers
            .iter_clients()
            .find(|server| server.name() == name)
            .unwrap()
            .id()
    }

    pub fn uri(&self) -> Uri {
        current_ref!(self.app.editor).1.uri().unwrap()
    }

    pub fn messages(&self) -> Vec<String> {
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
