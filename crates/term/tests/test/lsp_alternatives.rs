//! Selection and lifecycle behavior for ordered alternatives through real stdio clients.
use std::{collections::HashSet, path::Path, time::Duration};

use anyhow::Context as _;
use language::config::LanguageServerFeature;
use lsp_client::{Call, LanguageServerId};
use term::application::Application;
use tokio_stream::StreamExt;
use view::{current_ref, editor::Action};

use super::helpers::{
    lsp::{log_path, Gate, ServerConfig},
    test_config, test_syntax_loader, AppBuilder,
};

struct Fixture {
    app: Application,
    gate: Gate,
    initialized: HashSet<LanguageServerId>,
}

impl Fixture {
    fn new(
        dir: &Path,
        entries: &str,
        alpha_options: &[&str],
        alpha_settings: &str,
    ) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize-ready"));
        let mut alpha =
            ServerConfig::feature("alpha", "--language-features", dir).initialize_gate(&gate);
        for option in alpha_options {
            alpha = alpha.arg(option);
        }
        let mut alpha_definition: toml::Value = toml::from_str(&alpha.toml())?;
        let overrides: toml::Table = toml::from_str(alpha_settings)?;
        alpha_definition["language-server"]["alpha"]
            .as_table_mut()
            .unwrap()
            .extend(overrides);
        let definitions = format!(
            "{}\n{}\n{}",
            toml::to_string(&alpha_definition)?,
            ServerConfig::feature("beta", "--language-features", dir)
                .initialize_gate(&gate)
                .toml(),
            ServerConfig::feature("lint", "--language-features", dir)
                .initialize_gate(&gate)
                .toml()
        );
        let loader = test_syntax_loader(Some(format!(
            r#"
{definitions}
[[language]]
name = "alternatives-test"
scope = "source.alternatives-test"
file-types = ["alternatives-test"]
roots = []
language-servers = {entries}
"#
        )));
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        let path = dir.join("first.alternatives-test");
        std::fs::write(&path, "original\n")?;
        app.editor.open(&path, Action::Replace)?;
        Ok(Self {
            app,
            gate,
            initialized: HashSet::new(),
        })
    }

    fn active(&self) -> Vec<String> {
        current_ref!(self.app.editor)
            .1
            .language_servers()
            .map(|server| server.name().to_owned())
            .collect()
    }

    async fn next(&mut self) -> anyhow::Result<(LanguageServerId, Call)> {
        tokio::time::timeout(
            Duration::from_secs(10),
            self.app.editor.language_servers.incoming.next(),
        )
        .await?
        .context("server message stream closed")
    }

    async fn until_active(&mut self, names: &[&str]) -> anyhow::Result<()> {
        self.gate.release()?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.active() != names || !current_ref!(self.app.editor).1.language_servers()
                .all(|server| self.initialized.contains(&server.id()))
            {
                let (id, call) = self.next().await?;
                let initialized = matches!(&call, Call::Notification(notification) if notification.method == "initialized");
                self.app.handle_language_server_message(call, id).await;
                if initialized { self.initialized.insert(id); }
            }
            anyhow::Ok(())
        })
        .await??;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn starts_one_alternative_and_supplements_and_preserves_feature_filters() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(
        dir.path(),
        r#"[
        { alternatives = [{ name = "alpha", except-features = ["hover"] }, "beta"] },
        { name = "lint", only-features = ["hover"] },
    ]"#,
        &[],
        "",
    )?;
    f.until_active(&["alpha", "lint"]).await?;
    assert!(!log_path(dir.path(), "beta").exists());
    assert_eq!(
        current_ref!(f.app.editor)
            .1
            .language_servers_with_feature(LanguageServerFeature::Hover)
            .map(|server| server.name())
            .collect::<Vec<_>>(),
        ["lint"]
    );
    let second = dir.path().join("second.alternatives-test");
    std::fs::write(&second, "second\n")?;
    f.app.editor.open(&second, Action::Replace)?;
    assert_eq!(f.active(), ["alpha", "lint"]);
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 2);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_executables_and_inapplicable_projects_try_the_next_candidate() -> anyhow::Result<()>
{
    for missing in [true, false] {
        let dir = tempfile::tempdir()?;
        let settings = if missing {
            format!(
                "command = {}",
                toml::Value::String(
                    dir.path()
                        .join("missing-lsp")
                        .to_string_lossy()
                        .into_owned()
                )
            )
        } else {
            "required-root-patterns = ['__mitos_missing_project_marker__']".to_owned()
        };
        let mut f = Fixture::new(
            dir.path(),
            r#"[{ alternatives = ["alpha", "beta"] }]"#,
            &[],
            &settings,
        )?;
        f.until_active(&["beta"]).await?;
        assert!(!log_path(dir.path(), "alpha").exists());
        if missing {
            std::fs::copy(
                env!("CARGO_BIN_EXE_mitos-test-lsp"),
                dir.path().join("missing-lsp"),
            )?;
            let document = current_ref!(f.app.editor).1.id();
            f.app.editor.refresh_language_servers(document);
            assert_eq!(
                f.active(),
                ["beta"],
                "installing a preferred binary must not replace a running selection"
            );
            assert!(!log_path(dir.path(), "alpha").exists());
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_or_terminated_initialization_falls_back_for_all_attached_documents(
) -> anyhow::Result<()> {
    for failure in [
        "--reject-initialize",
        "--exit-on-initialize",
        "--ignore-initialize",
    ] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(
            dir.path(),
            r#"[{ alternatives = ["alpha", "beta"] }]"#,
            &[failure],
            "timeout = 1",
        )?;
        let second = dir.path().join("second.alternatives-test");
        std::fs::write(&second, "second\n")?;
        f.app.editor.open(&second, Action::Replace)?;
        f.until_active(&["beta"]).await?;
        assert!(f.app.editor.get_status().unwrap().0.contains("trying beta"));
        for doc in f.app.editor.documents().filter(|doc| {
            doc.path().is_some_and(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "alternatives-test")
            })
        }) {
            assert_eq!(
                doc.language_servers()
                    .map(|server| server.name())
                    .collect::<Vec<_>>(),
                ["beta"]
            );
        }
        assert_eq!(f.app.editor.language_servers.iter_clients().count(), 1);
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_blocks_fallback_and_restart_retries_preferences_or_explicit_choice(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(
        dir.path(),
        r#"[{ alternatives = ["alpha", "beta"] }]"#,
        &[],
        "",
    )?;
    f.until_active(&["alpha"]).await?;
    let document = current_ref!(f.app.editor).1.id();
    assert!(f
        .app
        .editor
        .restart_language_servers(document, &["alpha", "beta"])
        .is_err());
    assert_eq!(f.active(), ["alpha"]);
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 1);
    f.app.editor.stop_language_servers(document, &["alpha"])?;
    f.app.editor.refresh_language_servers(document);
    assert!(f.active().is_empty());
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 0);
    f.app.editor.restart_language_servers(document, &[])?;
    f.until_active(&["alpha"]).await?;
    f.app.editor.restart_language_servers(document, &["beta"])?;
    f.until_active(&["beta"]).await?;
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 1);
    f.app.editor.restart_language_servers(document, &[])?;
    f.until_active(&["alpha"]).await?;
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_servers_exit_does_not_switch_to_an_alternative() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(
        dir.path(),
        r#"[{ alternatives = ["alpha", "beta"] }]"#,
        &[],
        "",
    )?;
    f.until_active(&["alpha"]).await?;
    let server = current_ref!(f.app.editor)
        .1
        .language_servers()
        .next()
        .unwrap();
    let old = server.id();
    server.exit();
    loop {
        let (id, call) = f.next().await?;
        let exited = id == old
            && matches!(&call, Call::Notification(notification) if notification.method == "exit");
        f.app.handle_language_server_message(call, id).await;
        if exited {
            break;
        }
    }
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.refresh_language_servers(document);
    assert!(f.active().is_empty());
    assert!(!log_path(dir.path(), "beta").exists());
    f.app.editor.restart_language_servers(document, &[])?;
    f.until_active(&["alpha"]).await?;
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unrelated_running_server_does_not_override_another_languages_preferences(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let definitions = format!(
        "{}\n{}",
        ServerConfig::new("alpha").toml(),
        ServerConfig::new("beta").toml()
    );
    let loader = test_syntax_loader(Some(format!(
        r#"
{definitions}
[[language]]
name = "standalone-test"
scope = "source.standalone-test"
file-types = ["standalone-test"]
roots = []
language-servers = ["beta"]
[[language]]
name = "alternatives-test"
scope = "source.alternatives-test"
file-types = ["alternatives-test"]
roots = []
language-servers = [{{ alternatives = ["alpha", "beta"] }}]
"#
    )));
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(loader)
        .build()?;
    let mut f = Fixture {
        app,
        gate: Gate::new(dir.path().join("unused")),
        initialized: HashSet::new(),
    };
    let standalone = dir.path().join("first.standalone-test");
    std::fs::write(&standalone, "first\n")?;
    f.app.editor.open(&standalone, Action::Replace)?;
    f.until_active(&["beta"]).await?;
    let alternative = dir.path().join("second.alternatives-test");
    std::fs::write(&alternative, "second\n")?;
    f.app.editor.open(&alternative, Action::Replace)?;
    f.until_active(&["alpha"]).await?;
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 2);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_during_initialization_does_not_start_the_next_alternative() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(
        dir.path(),
        r#"[{ alternatives = ["alpha", "beta"] }]"#,
        &[],
        "",
    )?;
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.stop_language_servers(document, &["alpha"])?;
    f.gate.release()?;
    f.app.editor.refresh_language_servers(document);
    assert!(f.active().is_empty());
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 0);
    assert!(!log_path(dir.path(), "beta").exists());
    f.app.editor.restart_language_servers(document, &[])?;
    f.until_active(&["alpha"]).await?;
    assert!(f.app.close().await.is_empty());
    Ok(())
}
