//! Shared fake-server configuration and fixtures for LSP integration tests.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::Uri;
use lsp_client::{Call, LanguageServerId, Notification};
use term::application::Application;
use view::{current_ref, editor::Action};
use workspace::trust::WorkspaceTrust;

use super::{test_config, test_syntax_loader, AppBuilder};
pub use lsp_client::test_support::{log_path, read_log, wait_message, Gate, ServerConfig};

/// Give each fixture its own language while sharing the fake-server wiring.
pub fn syntax_loader(
    language: &str,
    file_type: &str,
    names: &[&str],
    servers: &str,
) -> language::syntax::Loader {
    test_syntax_loader(Some(lsp_client::test_support::language_config(
        language, file_type, names, servers,
    )))
}

type FeatureCallbacks =
    tokio::sync::mpsc::UnboundedReceiver<(bool, view::callbacks::EditorCallback)>;

pub fn feature_app<'a>(
    dir: &Path,
    text: &str,
    language: &str,
    names: &[&'a str],
    mut config: term::config::Config,
    server: impl Fn(&'a str) -> ServerConfig<'a>,
    setup: impl FnOnce(
            &mut view::handlers::Handlers,
            view::callbacks::EditorCallbackSender,
            &editor_config::Config,
        ) + 'static,
) -> anyhow::Result<(Application, FeatureCallbacks, Gate)> {
    let gate = Gate::new(dir.join("initialize-ready"));
    gate.close()?;
    let servers = names
        .iter()
        .map(|name| {
            server(name)
                .initialize_gate(&gate)
                .toml(env!("CARGO_BIN_EXE_mitos-test-lsp"))
        })
        .collect::<String>();
    config.editor.lsp.enable = true;
    let (sender, callbacks) = super::callbacks::deferred(|blocking, callback| (blocking, callback));
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(syntax_loader(language, language, names, &servers))
        .with_handler_setup(move |handlers, config| {
            let sender = sender(handlers.redraw().clone());
            setup(handlers, sender, config);
        })
        .build()?;
    let path = dir.join(format!("document.{language}"));
    std::fs::write(&path, text)?;
    app.editor.open(&path, Action::Replace)?;
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(
        view.id,
        editor_core::Selection::point(doc.text().len_chars() - 1),
    );
    Ok((app, callbacks, gate))
}

/// Wait for editor-side initialization, not just transport readiness. Processing
/// the notifications sends didOpen before a test starts observing feature requests.
pub async fn initialize(app: &mut Application, servers: usize) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        for _ in 0..servers {
            let (server, call) = app
                .editor
                .next_language_server_message()
                .await
                .context("LSP message stream closed during initialization")?;
            anyhow::ensure!(
                matches!(&call, Call::Notification(message) if message.method == "initialized"),
                "expected initialization notification from {server:?}, got {call:?}"
            );
            app.handle_language_server_message(call, server).await;
        }
        anyhow::Ok(())
    })
    .await
    .context("test language servers did not initialize")?
}

pub(crate) struct Fixture {
    pub app: Application,
    gate: Gate,
    pub logs: Vec<PathBuf>,
}

impl Fixture {
    pub fn new(dir: &Path, names: &[&str]) -> anyhow::Result<Self> {
        Self::with_config(dir, names, |name| {
            Some(format!("{{ lifecycle = \"{name}\" }}"))
        })
    }

    pub fn with_events(
        dir: &Path,
        names: &[&str],
        events: event::EventHub,
    ) -> anyhow::Result<Self> {
        Self::with_config_and_trust(
            dir,
            names,
            |name| Some(format!("{{ lifecycle = \"{name}\" }}")),
            WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
            events,
        )
    }

    pub fn with_config(
        dir: &Path,
        names: &[&str],
        config: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        Self::with_config_and_trust(
            dir,
            names,
            config,
            WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
            event::EventHub::default(),
        )
    }

    pub fn with_trust(dir: &Path, names: &[&str], trust: WorkspaceTrust) -> anyhow::Result<Self> {
        Self::with_config_and_trust(dir, names, |_| None, trust, event::EventHub::default())
    }

    fn with_config_and_trust(
        dir: &Path,
        names: &[&str],
        config: impl Fn(&str) -> Option<String>,
        trust: WorkspaceTrust,
        events: event::EventHub,
    ) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize-ready"));
        let mut servers = String::new();
        let mut logs = Vec::new();
        for name in names {
            // Lifecycle mode logs initialize before waiting on its positional gate.
            servers.push_str(
                &ServerConfig::feature(name, "--lifecycle", dir)
                    .arg(gate.path())
                    .toml(env!("CARGO_BIN_EXE_mitos-test-lsp")),
            );
            if let Some(config) = config(name) {
                servers.push_str(&format!("config = {config}\n"));
            }
            logs.push(log_path(dir, name));
        }
        let loader = syntax_loader("lifecycle-test", "lifecycle-test", names, &servers);
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.breadcrumb.enable = true;
        let mut app = AppBuilder::new()
            .with_events(events)
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        app.editor.workspace_trust = trust;
        let path = dir.join("document.lifecycle-test");
        std::fs::write(&path, "😀 original\n")?;
        app.editor.open(&path, Action::Replace)?;
        Ok(Self { app, gate, logs })
    }

    pub fn release(&self) -> anyhow::Result<()> {
        self.gate.release()
    }

    pub async fn next(&mut self) -> anyhow::Result<(LanguageServerId, Call)> {
        tokio::time::timeout(
            Duration::from_secs(10),
            self.app.editor.next_language_server_message(),
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
            .language_servers()
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
