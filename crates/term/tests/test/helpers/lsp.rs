//! Shared fake-server configuration and fixtures for LSP integration tests.
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::Uri;
use lsp_client::{Call, LanguageServerId, Notification};
use term::application::TestApplication;
use view::{current_ref, editor::Action};
use workspace::trust::WorkspaceTrust;

use super::{test_config, test_syntax_loader, AppBuilder};

/// A file gate keeps server startup under the test's control.
pub struct Gate(PathBuf);

impl Gate {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn close(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(&self.0) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn release(&self) -> anyhow::Result<()> {
        std::fs::write(&self.0, "ready")?;
        Ok(())
    }
}

pub fn log_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}.jsonl"))
}

/// Read complete JSON records while the fixture may still be appending to its log.
/// An unfinished trailing record can also end in the middle of a UTF-8 character.
pub fn read_log(path: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    std::fs::read(path)?
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|record| record.ends_with(b"\n"))
        .map(serde_json::from_slice)
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

#[test]
fn log_snapshots_wait_for_complete_records_and_report_invalid_json() -> anyhow::Result<()> {
    use std::io::Write;
    let mut log = tempfile::NamedTempFile::new()?;
    log.write_all(b"{\"phase\":\"ready\"}\n{\"phase\":\"\xe7")?;
    assert_eq!(
        read_log(log.path())?,
        vec![serde_json::json!({"phase": "ready"})]
    );
    log.write_all(b"\x95\x8c\"}\n")?;
    assert_eq!(
        read_log(log.path())?,
        vec![
            serde_json::json!({"phase": "ready"}),
            serde_json::json!({"phase": "界"})
        ]
    );
    log.write_all(b"invalid\n")?;
    assert!(read_log(log.path()).is_err());
    Ok(())
}

/// Configure the existing stdio test server. Feature modes require the mode,
/// server name, and log path in that order; additional arguments stay explicit.
pub struct ServerConfig<'a> {
    name: &'a str,
    args: Vec<String>,
}

impl<'a> ServerConfig<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            args: Vec::new(),
        }
    }

    pub fn feature(name: &'a str, mode: &str, directory: &Path) -> Self {
        Self::new(name)
            .arg(mode)
            .arg(name)
            .arg(log_path(directory, name))
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(
            arg.as_ref()
                .to_str()
                .expect("UTF-8 test server argument")
                .into(),
        );
        self
    }

    pub fn initialize_gate(self, gate: &Gate) -> Self {
        self.arg("--initialize-gate").arg(gate.path())
    }

    pub fn toml(self) -> String {
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-test-lsp").into());
        let args = string_array(&self.args);
        let name = toml::Value::String(self.name.into());
        format!("[language-server.{name}]\ncommand = {command}\nargs = {args}\n")
    }
}

fn string_array(values: &[impl AsRef<str>]) -> toml::Value {
    toml::Value::Array(
        values
            .iter()
            .map(|value| toml::Value::String(value.as_ref().into()))
            .collect(),
    )
}

/// Give each fixture its own language while sharing the fake-server wiring.
pub fn syntax_loader(
    language: &str,
    file_type: &str,
    names: &[&str],
    servers: &str,
) -> language::syntax::Loader {
    let name = toml::Value::String(language.into());
    let scope = toml::Value::String(format!("source.{language}"));
    let file_types = string_array(&[file_type]);
    let names = string_array(names);
    test_syntax_loader(Some(format!(
        r#"
        {servers}
        [[language]]
        name = {name}
        scope = {scope}
        file-types = {file_types}
        roots = []
        language-servers = {names}
        "#
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
) -> anyhow::Result<(TestApplication, FeatureCallbacks, Gate)> {
    let gate = Gate::new(dir.join("initialize-ready"));
    gate.close()?;
    let servers = names
        .iter()
        .map(|name| server(name).initialize_gate(&gate).toml())
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

pub async fn wait_message(path: &Path, method: &str) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records = match read_log(path) {
                Ok(records) => records,
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    Vec::new()
                }
                Err(error) => return Err(error),
            };
            if let Some(message) = records
                .into_iter()
                .find(|message| message["method"] == method)
            {
                return Ok(message);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .with_context(|| format!("server did not receive {method}"))?
}

/// Wait for editor-side initialization, not just transport readiness. Processing
/// the notifications sends didOpen before a test starts observing feature requests.
pub async fn initialize(app: &mut TestApplication, servers: usize) -> anyhow::Result<()> {
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
    pub app: TestApplication,
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
                    .toml(),
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
