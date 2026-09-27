use std::{path::PathBuf, process::Command};

use loader::workspace_trust::{Config, TrustStatus, WorkspaceTrust};
use view::{
    current_ref,
    editor::{Action, ConfigEvent, EditorEvent},
    events::DocumentDidOpen,
    handlers::workspace_trust::{
        apply_decision, next_request, resolve_request, trust_and_restart, TrustDecision,
    },
};

use super::helpers::{lsp::Fixture, test_key_sequence, test_key_sequences, AppBuilder};

// Trust persistence and config discovery use process-wide paths. Each child gets
// its own data and configuration directories, never the developer's trust store.
fn isolated(name: &str) -> anyhow::Result<Option<PathBuf>> {
    const ROOT: &str = "MITOS_TEST_WORKSPACE_TRUST_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = PathBuf::from(root);
        assert!(loader::data_dir().starts_with(&root));
        assert!(loader::config_dir().starts_with(&root));
        loader::initialize_config_file(Some(root.join("config/mitos/config.toml")));
        return Ok(Some(root));
    }
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    for path in ["workspace/.mitos", "config/mitos", "data", "cache"] {
        std::fs::create_dir_all(root.join(path))?;
    }
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            &format!("test::workspace_trust::{name}"),
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(root.join("workspace"))
        .env(
            "MITOS_RUNTIME",
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime"),
        )
        .env(ROOT, &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("data"))
        .env("APPDATA", root.join("config"))
        .env("LOCALAPPDATA", root.join("cache"))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "isolated test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(None)
}

fn opened(editor: &mut view::Editor) {
    let doc = current_ref!(editor).1.id();
    event::dispatch(DocumentDidOpen { editor, doc });
}

fn drain_theme_events(editor: &mut view::Editor) {
    while let Ok(event) = editor.config_events.1.try_recv() {
        assert!(matches!(event, ConfigEvent::ThemeChanged));
    }
}

fn app(root: &std::path::Path) -> anyhow::Result<term::application::Application> {
    std::fs::write(
        root.join("config/mitos/config.toml"),
        r#"
        [editor]
        scrolloff = 7
        [editor.lsp]
        enable = false
        [editor.file-watcher]
        enable = false
        watch-vcs = false
        [editor.auto-reload]
        enable = false
        [editor.word-completion]
        enable = false
        [editor.workspace-trust]
        level = "none"
    "#,
    )?;
    std::fs::write(
        root.join("config/mitos/languages.toml"),
        r#"
        [[language]]
        name = "trust-test"
        scope = "source.trust-test"
        file-types = ["trust-test"]
        roots = []
        language-servers = []
    "#,
    )?;
    std::fs::write(
        root.join("workspace/.mitos/config.toml"),
        "[editor]\nscrolloff = 17\n",
    )?;
    let path = root.join("workspace/document.trust-test");
    std::fs::write(&path, "text\n")?;
    let mut app = AppBuilder::new().with_file(path, None).build()?;
    app.handle_config_events(ConfigEvent::Refresh);
    assert_eq!(app.editor.config().scrolloff, 7);
    opened(&mut app.editor);
    drain_theme_events(&mut app.editor);
    Ok(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn modal_and_commands_apply_trust_and_reload_workspace_configuration() -> anyhow::Result<()> {
    let Some(root) = isolated("modal_and_commands_apply_trust_and_reload_workspace_configuration")?
    else {
        return Ok(());
    };
    let mut app = app(&root)?;
    let workspace = root.join("workspace");
    let request = next_request(&mut app.editor).unwrap();
    app.handle_editor_event(EditorEvent::WorkspaceTrust(request))
        .await;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<ret>"),
                Some(&|app| {
                    assert_eq!(
                        app.editor.workspace_trust.status(&workspace),
                        TrustStatus::Trusted
                    );
                    assert_eq!(app.editor.config().scrolloff, 17);
                }),
            ),
            (
                Some(":workspace-untrust<ret>"),
                Some(&|app| {
                    assert_eq!(
                        app.editor.workspace_trust.status(&workspace),
                        TrustStatus::Untrusted
                    );
                    assert_eq!(app.editor.config().scrolloff, 7);
                }),
            ),
            (
                Some(":workspace-trust<ret>"),
                Some(&|app| {
                    assert_eq!(
                        app.editor.workspace_trust.status(&workspace),
                        TrustStatus::Trusted
                    );
                    assert_eq!(app.editor.config().scrolloff, 17);
                }),
            ),
            (
                Some(":workspace-exclude<ret>"),
                Some(&|app| {
                    assert_eq!(
                        app.editor.workspace_trust.status(&workspace),
                        TrustStatus::Excluded
                    );
                    assert_eq!(app.editor.config().scrolloff, 7);
                }),
            ),
        ],
        false,
    )
    .await?;
    assert_eq!(
        WorkspaceTrust::new(Config::default()).status(&workspace),
        TrustStatus::Excluded
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn modal_never_persists_exclusion() -> anyhow::Result<()> {
    let Some(root) = isolated("modal_never_persists_exclusion")? else {
        return Ok(());
    };
    let mut app = app(&root)?;
    let request = next_request(&mut app.editor).unwrap();
    app.handle_editor_event(EditorEvent::WorkspaceTrust(request))
        .await;
    test_key_sequence(
        &mut app,
        Some("<down><ret>"),
        Some(&|app| {
            assert_eq!(
                app.editor.workspace_trust.status(&root.join("workspace")),
                TrustStatus::Excluded
            );
            assert_eq!(app.editor.config().scrolloff, 7);
        }),
        false,
    )
    .await?;
    assert_eq!(
        WorkspaceTrust::new(Config::default()).status(&root.join("workspace")),
        TrustStatus::Excluded
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn modal_escape_dismisses_without_persisting() -> anyhow::Result<()> {
    let Some(root) = isolated("modal_escape_dismisses_without_persisting")? else {
        return Ok(());
    };
    let mut app = app(&root)?;
    let request = next_request(&mut app.editor).unwrap();
    app.handle_editor_event(EditorEvent::WorkspaceTrust(request))
        .await;
    test_key_sequence(
        &mut app,
        Some("<esc>"),
        Some(&|app| {
            assert_eq!(
                app.editor.workspace_trust.status(&root.join("workspace")),
                TrustStatus::Untrusted
            );
            assert_eq!(app.editor.config().scrolloff, 7);
        }),
        false,
    )
    .await?;
    assert!(!loader::data_dir().join("workspace_trust").exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_decisions_preserve_launch_restart_revocation_and_stale_semantics(
) -> anyhow::Result<()> {
    let Some(root) =
        isolated("shared_decisions_preserve_launch_restart_revocation_and_stale_semantics")?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    let local = workspace.join(".mitos/config.toml");
    std::fs::write(&local, "")?;
    let mut f = Fixture::new(&workspace, &["alpha", "beta"])?;
    f.initialize().await?;
    drain_theme_events(&mut f.app.editor);
    let first = current_ref!(f.app.editor).1.id();
    let path = workspace.join("other.lifecycle-test");
    std::fs::write(&path, "other\n")?;
    let second = f.app.editor.open(&path, Action::Load)?;
    let original = [f.server("alpha"), f.server("beta")];
    f.app.editor.workspace_trust.set_config(Config::default());
    opened(&mut f.app.editor);
    let request = next_request(&mut f.app.editor).unwrap();
    assert!(resolve_request(
        &mut f.app.editor,
        &request,
        TrustDecision::Trust
    )?);
    assert_eq!([f.server("alpha"), f.server("beta")], original);
    assert!(matches!(
        f.app.editor.config_events.1.try_recv(),
        Ok(ConfigEvent::Refresh)
    ));
    assert!(!resolve_request(
        &mut f.app.editor,
        &request,
        TrustDecision::Exclude
    )?);
    assert!(f.app.editor.config_events.1.try_recv().is_err());
    assert_eq!(
        WorkspaceTrust::new(Config::default()).status(&workspace),
        TrustStatus::Trusted
    );

    trust_and_restart(&mut f.app.editor, first, &["alpha"])?;
    let restarted = f.server("alpha");
    assert_ne!(restarted, original[0]);
    assert_eq!(f.server("beta"), original[1]);
    loop {
        let (server, call) = f.next().await?;
        let ready = server == restarted
            && matches!(&call, lsp_client::Call::Notification(notification) if notification.method == "initialized");
        f.app.handle_language_server_message(call, server).await;
        if ready {
            break;
        }
    }
    for doc in [first, second] {
        assert!(f
            .app
            .editor
            .document(doc)
            .unwrap()
            .language_servers()
            .any(|ls| ls.id() == restarted));
    }
    assert!(matches!(
        f.app.editor.config_events.1.try_recv(),
        Ok(ConfigEvent::Refresh)
    ));
    let error = trust_and_restart(&mut f.app.editor, first, &["unknown"]).unwrap_err();
    assert_eq!(error.to_string(), "Unknown language server: unknown");
    // Trust and refresh still precede restart errors, as in the original command.
    assert!(matches!(
        f.app.editor.config_events.1.try_recv(),
        Ok(ConfigEvent::Refresh)
    ));

    std::fs::write(&local, "[editor]\nmouse = false\n")?;
    f.app.editor.workspace_trust.set_config(Config::default());
    opened(&mut f.app.editor);
    assert_eq!(
        f.app.editor.workspace_trust.status(&workspace),
        TrustStatus::Stale
    );
    assert!(f
        .app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("config changed"));
    assert!(next_request(&mut f.app.editor).is_none());
    for decision in [TrustDecision::Untrust, TrustDecision::Exclude] {
        apply_decision(&mut f.app.editor, &workspace, decision)?;
        assert_eq!(
            [f.server("alpha"), f.server("beta")],
            [restarted, original[1]]
        );
        assert!(matches!(
            f.app.editor.config_events.1.try_recv(),
            Ok(ConfigEvent::Refresh)
        ));
    }
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_decisions_invalidate_queued_and_visible_prompts() -> anyhow::Result<()> {
    let Some(root) = isolated("explicit_decisions_invalidate_queued_and_visible_prompts")? else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    for decision in [
        TrustDecision::Trust,
        TrustDecision::Untrust,
        TrustDecision::Exclude,
    ] {
        for delivered in [false, true] {
            WorkspaceTrust::new(Config::default()).untrust(&workspace);
            let mut app = app(&root)?;
            let request = delivered.then(|| next_request(&mut app.editor).unwrap());
            apply_decision(&mut app.editor, &workspace, decision)?;
            assert!(matches!(
                app.editor.config_events.1.try_recv(),
                Ok(ConfigEvent::Refresh)
            ));
            if let Some(request) = request {
                assert!(!resolve_request(
                    &mut app.editor,
                    &request,
                    TrustDecision::Trust
                )?);
            } else {
                assert!(next_request(&mut app.editor).is_none());
            }
            assert!(app.editor.config_events.1.try_recv().is_err());
            assert!(app.close().await.is_empty());
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn accepting_trust_launches_previously_blocked_servers_for_open_documents(
) -> anyhow::Result<()> {
    let Some(root) =
        isolated("accepting_trust_launches_previously_blocked_servers_for_open_documents")?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    let mut f = Fixture::with_trust(
        &workspace,
        &["alpha", "beta"],
        WorkspaceTrust::new(Config {
            level: loader::workspace_trust::ImplicitTrustLevel::None,
            ..Config::default()
        }),
    )?;
    let first = current_ref!(f.app.editor).1.id();
    let path = workspace.join("other.lifecycle-test");
    std::fs::write(&path, "other\n")?;
    let second = f.app.editor.open(&path, Action::Load)?;
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 0);
    let request = next_request(&mut f.app.editor).unwrap();
    assert!(next_request(&mut f.app.editor).is_none());
    assert!(resolve_request(
        &mut f.app.editor,
        &request,
        TrustDecision::Trust
    )?);
    f.initialize().await?;
    assert_eq!(f.app.editor.language_servers.iter_clients().count(), 2);
    for doc in [first, second] {
        let doc = f.app.editor.document(doc).unwrap();
        assert_eq!(doc.language_servers().count(), 2);
        for server in [f.server("alpha"), f.server("beta")] {
            assert!(doc.language_servers().any(|ls| ls.id() == server));
        }
    }
    assert!(f.app.close().await.is_empty());
    Ok(())
}
