use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use serde_json::json;
use view::{
    config::ImplicitTrustLevelConfig,
    current_ref,
    editor::{Action, ConfigEvent},
    events::ConfigDidChange,
    theme::Theme,
};
use workspace::trust::{ImplicitTrustLevel, TrustQuery};

use super::helpers::{lsp::Fixture, test_syntax_loader, AppBuilder};

#[tokio::test(flavor = "multi_thread")]
async fn shared_language_application_updates_scopes_documents_and_diagnostics() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    f.initialize().await?;
    let first = current_ref!(f.app.editor).1.id();
    let other = dir.path().join("other.lifecycle-test");
    std::fs::write(&other, "{}\n")?;
    let second = f.app.editor.open(&other, Action::Load)?;
    let server = f.server("alpha");
    for id in [first, second] {
        let doc = f.app.editor.document(id).unwrap();
        let params = serde_json::from_value(json!({
            "uri": doc.url().unwrap(), "version": doc.version(),
            "diagnostics": [{"message": "cached", "range": {
                "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}
            }}]
        }))?;
        f.app.editor.handle_publish_diagnostics(server, params);
        assert_eq!(f.app.editor.document(id).unwrap().diagnostics().len(), 1);
    }
    std::fs::write(
        dir.path().join(".editorconfig"),
        "root = true\n[*]\nmax_line_length = 61\n",
    )?;
    let loader = test_syntax_loader(Some(
        r#"
        [[language]]
        name = "reloaded-config-test"
        scope = "source.reloaded-config-test"
        file-types = ["lifecycle-test"]
        roots = []
        grammar = "json"
        language-servers = [{ name = "alpha", except-features = ["diagnostics"] }]
    "#
        .into(),
    ));
    let theme = Theme::from(toml::from_str::<toml::Value>(
        r#"
        "ui.selection" = { bg = "blue" }
        "string" = "green"
    "#,
    )?);
    let scopes = theme.scopes().to_vec();
    f.app.editor.apply_language_config(loader, theme)?;
    assert_eq!(f.app.editor.syn_loader.load().scopes().as_slice(), scopes);
    for id in [first, second] {
        let doc = f.app.editor.document(id).unwrap();
        assert_eq!(
            doc.language_config().unwrap().language_id,
            "reloaded-config-test"
        );
        assert_eq!(doc.text_width(), 61);
        assert!(doc.diagnostics().is_empty());
        assert!(doc.is_syntax_pending());
        // Refresh filters the document projection, without discarding the cached reports.
        assert_eq!(f.app.editor.diagnostics[&doc.uri().unwrap()].len(), 1);
    }
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_loading_applies_new_trust_before_reading_local_files() -> anyhow::Result<()> {
    let Some(root) = super::helpers::isolation::workspace(
        "test::config_application::language_loading_applies_new_trust_before_reading_local_files",
    )?
    else {
        return Ok(());
    };
    let language = |width| {
        format!(
            r#"
        [[language]]
        name = "trust-config-test"
        scope = "source.trust-config-test"
        file-types = ["trust-config-test"]
        roots = []
        text-width = {width}
    "#
        )
    };
    std::fs::write(root.join("config/mitos/languages.toml"), language(71))?;
    let local = root.join("workspace/.mitos/languages.toml");
    std::fs::write(&local, language(99))?;
    let mut app = AppBuilder::new().build()?;
    let active = app.editor.syn_loader.load_full();
    let mut config = (*app.editor.config()).clone();
    config.workspace_trust.level = ImplicitTrustLevelConfig::None;
    let loader = app.editor.load_language_config(&config)?;
    let width = |loader: &editor_core::syntax::Loader| {
        loader
            .language_configs()
            .find(|language| language.language_id == "trust-config-test")
            .unwrap()
            .text_width
    };
    assert_eq!(width(&loader), Some(71));
    assert!(!app
        .editor
        .workspace_trust
        .query_current(TrustQuery::LocalConfig)
        .is_trusted());
    config.workspace_trust.level = ImplicitTrustLevelConfig::Insecure;
    let loader = app.editor.load_language_config(&config)?;
    assert_eq!(width(&loader), Some(99));
    assert!(Arc::ptr_eq(&active, &app.editor.syn_loader.load_full()));
    std::fs::write(&local, "invalid = [")?;
    config.workspace_trust.level = ImplicitTrustLevelConfig::None;
    assert!(app.editor.load_language_config(&config).is_ok());
    config.workspace_trust.level = ImplicitTrustLevelConfig::Insecure;
    assert!(app.editor.load_language_config(&config).is_err());
    assert!(matches!(
        app.editor.workspace_trust.implicit_level(),
        ImplicitTrustLevel::Insecure
    ));
    assert!(Arc::ptr_eq(&active, &app.editor.syn_loader.load_full()));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn application_reload_publishes_settings_after_resources_and_preserves_failure_boundaries(
) -> anyhow::Result<()> {
    let Some(root) = super::helpers::isolation::workspace(
        "test::config_application::application_reload_publishes_settings_after_resources_and_preserves_failure_boundaries",
    )?
    else {
        return Ok(());
    };
    let config_path = root.join("config/mitos/config.toml");
    let config = r#"
        theme = "base16_default"
        [editor.scroll]
        padding = 7
        [editor.lsp]
        enable = false
        [editor.files.watch]
        enable = false
        watch-vcs = false
        [editor.files.reload]
        enable = false
        [editor.completion.word]
        enable = false
        [editor.workspace-trust]
        level = "insecure"
        prompt = false
    "#;
    std::fs::write(&config_path, config)?;
    let local = root.join("workspace/.mitos/languages.toml");
    std::fs::write(
        &local,
        r#"
        [[language]]
        name = "reload-config-test"
        scope = "source.reload-config-test"
        file-types = ["reload-config-test"]
        roots = []
    "#,
    )?;
    let path = root.join("workspace/document.reload-config-test");
    std::fs::write(&path, "text\n")?;
    let mut app = AppBuilder::new().with_file(path, None).build()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    event::register_hook!(move |event: &mut ConfigDidChange<'_>| {
        assert_eq!(event.new.scroll.padding, 7);
        assert_eq!(event.editor.config().scroll.padding, 7);
        assert_eq!(
            current_ref!(event.editor)
                .1
                .language_config()
                .unwrap()
                .language_id,
            "reload-config-test"
        );
        assert_eq!(
            event.editor.syn_loader.load().scopes().as_slice(),
            event.editor.theme.scopes()
        );
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    app.handle_config_events(ConfigEvent::Refresh);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.editor.get_status().unwrap().0, "Config refreshed");
    let active = app.editor.syn_loader.load_full();
    // A malformed application config fails before trust or language application.
    std::fs::write(&config_path, "invalid = [")?;
    app.handle_config_events(ConfigEvent::Refresh);
    assert!(app
        .editor
        .get_status()
        .unwrap()
        .0
        .starts_with("Failed to load config:"));
    assert_eq!(calls.load(Ordering::SeqCst), 2); // Existing behavior: refresh hooks still run.
    assert!(Arc::ptr_eq(&active, &app.editor.syn_loader.load_full()));
    // A malformed global language config fails after the new trust policy is applied,
    // but before replacing the active settings or language loader.
    std::fs::write(
        &config_path,
        config
            .replace("insecure", "none")
            .replace("padding = 7", "padding = 9"),
    )?;
    std::fs::write(root.join("config/mitos/languages.toml"), "invalid = [")?;
    app.handle_config_events(ConfigEvent::Refresh);
    assert!(app
        .editor
        .get_status()
        .unwrap()
        .0
        .starts_with("Failed to parse language config:"));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(app.editor.config().scroll.padding, 7);
    assert!(matches!(
        app.editor.workspace_trust.implicit_level(),
        ImplicitTrustLevel::None
    ));
    assert!(Arc::ptr_eq(&active, &app.editor.syn_loader.load_full()));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn theme_completion_and_commands_use_each_editors_selected_resources() -> anyhow::Result<()> {
    use super::helpers::{test_config, test_key_sequence, test_key_sequences};
    use loader::theme::Resources;
    use view::theme::{Color, Loader, Style};

    fn write(root: &std::path::Path, name: &str, color: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(root.join("themes"))?;
        std::fs::write(
            root.join("themes").join(format!("{name}.toml")),
            format!("inherits = 'default'\nkeyword = '{color}'"),
        )?;
        Ok(())
    }
    fn names(editor: &view::Editor) -> Vec<String> {
        let mut names: Vec<_> = term::ui::completers::theme(editor, "")
            .into_iter()
            .map(|(_, span)| span.content.into_owned())
            .collect();
        names.sort();
        names
    }
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    write(a.path(), "shared", "blue")?;
    write(a.path(), "only-a", "blue")?;
    write(b.path(), "shared", "red")?;
    write(b.path(), "only-b", "red")?;
    let mut config = test_config();
    config.terminal.true_color = true;
    let mut first = AppBuilder::new().with_config(config.clone()).build()?;
    let mut second = AppBuilder::new().with_config(config).build()?;
    first.editor.theme_loader = Arc::new(Loader::new(Resources::new(vec![a.path().into()])));
    second.editor.theme_loader = Arc::new(Loader::new(Resources::new(vec![b.path().into()])));
    assert_eq!(
        names(&first.editor),
        ["base16_default", "default", "only-a", "shared"]
    );
    assert_eq!(
        names(&second.editor),
        ["base16_default", "default", "only-b", "shared"]
    );
    test_key_sequence(&mut second, Some(":theme shared<ret>"), None, false).await?;
    assert_eq!(
        second.editor.theme.get("keyword"),
        Style::default().fg(Color::Red)
    );
    test_key_sequences(
        &mut first,
        vec![
            (
                Some(":theme shared<ret>"),
                Some(&|app: &term::application::Application| {
                    assert_eq!(app.editor.theme.name(), "shared");
                    assert_eq!(
                        app.editor.theme.get("keyword"),
                        Style::default().fg(Color::Blue)
                    );
                    write(a.path(), "added", "green").unwrap();
                    assert!(names(&app.editor).contains(&"added".into()));
                    assert!(!names(&second.editor).contains(&"added".into()));
                    write(a.path(), "shared", "green").unwrap();
                }),
            ),
            (
                Some(":theme shared<ret>"),
                Some(&|app: &term::application::Application| {
                    assert_eq!(
                        app.editor.theme.get("keyword"),
                        Style::default().fg(Color::Green)
                    );
                    std::fs::write(a.path().join("themes/shared.toml"), "inherits = 'missing'")
                        .unwrap();
                }),
            ),
            (
                Some(":theme shared<ret>"),
                Some(&|app: &term::application::Application| {
                    assert_eq!(
                        app.editor.theme.get("keyword"),
                        Style::default().fg(Color::Green)
                    );
                    assert!(app
                        .editor
                        .get_status()
                        .unwrap()
                        .0
                        .contains("Could not load theme"));
                    assert_eq!(
                        second.editor.theme.get("keyword"),
                        Style::default().fg(Color::Red)
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn config_reload_preserves_legacy_user_and_trusted_project_configs() -> anyhow::Result<()> {
    use super::helpers::{isolation, test_key_sequences};
    use workspace::trust::{Config as TrustConfig, TrustStatus, WorkspaceTrust};
    let Some(root) = isolation::workspace(
        "test::config_application::config_reload_preserves_legacy_user_and_trusted_project_configs",
    )?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    let user = loader::config_file();
    let project = loader::workspace_config_file();
    let global = "[editor]\nscrolloff = 7\n[editor.workspace-trust]\nlevel = 'none'\n";
    let local = "# Project settings\n[editor]\nscrolloff = 17\n";
    std::fs::write(&user, global)?;
    std::fs::write(&project, local)?;
    WorkspaceTrust::new(TrustConfig::default(), loader::workspace_trust_dir()).trust(&workspace);
    let mut app = AppBuilder::new().build()?;
    let check = |app: &term::application::Application| {
        assert_eq!(app.editor.config().scroll.padding, 17);
        assert_eq!(app.editor.get_status().unwrap().0, "Config refreshed");
        assert_eq!(
            WorkspaceTrust::new(TrustConfig::default(), loader::workspace_trust_dir())
                .status(&workspace),
            TrustStatus::Trusted
        );
        assert_eq!(std::fs::read_to_string(&user).unwrap(), global);
        assert_eq!(std::fs::read_to_string(&project).unwrap(), local);
        assert!(!user.with_extension("toml.bak").exists());
        assert!(!project.with_extension("toml.bak").exists());
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some(":config-reload<ret>"), Some(&check)),
            (Some(":config-reload<ret>"), Some(&check)),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn migration_dialog_cancels_rechecks_and_migrates_only_the_confirmed_scope(
) -> anyhow::Result<()> {
    use super::helpers::{isolation, test_key_sequences};
    use workspace::trust::{Config as TrustConfig, TrustStatus, WorkspaceTrust};
    let Some(root) = isolation::workspace(
        "test::config_application::migration_dialog_cancels_rechecks_and_migrates_only_the_confirmed_scope",
    )? else {
        return Ok(());
    };
    let user = loader::config_file();
    let project = loader::workspace_config_file();
    let global = "[editor]\nscrolloff = 7\n[editor.workspace-trust]\nlevel = 'none'\n";
    let local = "[editor]\nscrolloff = 17\n";
    std::fs::write(&user, global)?;
    std::fs::write(&project, local)?;
    let workspace = root.join("workspace");
    WorkspaceTrust::new(TrustConfig::default(), loader::workspace_trust_dir()).trust(&workspace);
    let mut app = AppBuilder::new().build()?;
    let unchanged = |_: &term::application::Application| {
        assert_eq!(std::fs::read_to_string(&user).unwrap(), global);
        assert!(!user.with_extension("toml.bak").exists());
    };
    let tamper = |app: &term::application::Application| {
        unchanged(app);
        std::fs::write(&user, global.replace("7", "9")).unwrap();
    };
    let unsaved = |app: &term::application::Application| {
        assert!(app
            .editor
            .get_status()
            .unwrap()
            .0
            .contains("unsaved configuration changes"));
        unchanged(app);
    };
    let rejected = |app: &term::application::Application| {
        assert!(app
            .editor
            .get_status()
            .unwrap()
            .0
            .contains("changed while awaiting confirmation"));
        assert_eq!(
            std::fs::read_to_string(&user).unwrap(),
            global.replace("7", "9")
        );
        assert!(!user.with_extension("toml.bak").exists());
        std::fs::write(&user, global).unwrap();
    };
    let user_migrated = |app: &term::application::Application| {
        assert!(app.editor.get_status().unwrap().0.contains("backup:"));
        assert_eq!(
            std::fs::read_to_string(user.with_extension("toml.bak")).unwrap(),
            global
        );
        assert_eq!(std::fs::read_to_string(&project).unwrap(), local);
        assert!(!project.with_extension("toml.bak").exists());
    };
    let workspace_migrated = |_: &term::application::Application| {
        assert_eq!(
            std::fs::read_to_string(project.with_extension("toml.bak")).unwrap(),
            local
        );
        assert_eq!(
            WorkspaceTrust::new(TrustConfig::default(), loader::workspace_trust_dir())
                .status(&workspace),
            TrustStatus::Trusted
        );
    };
    let canonical = |app: &term::application::Application| {
        assert_eq!(
            app.editor.get_status().unwrap().0,
            "Configuration already uses current settings"
        );
        assert!(!user.with_extension("toml.bak.1").exists());
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some(":config-migrate<ret>"), Some(&unchanged)),
            (Some("<ret>"), Some(&unchanged)),
            (Some(":config-migrate<ret>"), None),
            (Some("<esc>"), Some(&unchanged)),
            (Some(":config-open<ret>i# unsaved<esc>"), None),
            (Some(":config-migrate<ret>"), None),
            (Some("<down><ret>"), Some(&unsaved)),
            (Some(":reload<ret>"), None),
            (Some(":config-migrate<ret>"), Some(&tamper)),
            (Some("<down><ret>"), Some(&rejected)),
            (Some(":config-migrate<ret>"), None),
            (Some("<down><ret>"), Some(&user_migrated)),
            (Some(":config-migrate-workspace<ret>"), None),
            (Some("<down><ret>"), Some(&workspace_migrated)),
            (Some(":config-migrate<ret>"), Some(&canonical)),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn migration_rejects_unsaved_config_opened_through_symlink() -> anyhow::Result<()> {
    let Some(root) = super::helpers::isolation::workspace(
        "test::config_application::migration_rejects_unsaved_config_opened_through_symlink",
    )?
    else {
        return Ok(());
    };
    let user = loader::config_file();
    let original = "[editor]\nscrolloff = 7\n";
    std::fs::write(&user, original)?;
    let alias = root.join("config-link.toml");
    std::os::unix::fs::symlink(&user, &alias)?;
    let mut app = AppBuilder::new().with_file(&alias, None).build()?;
    super::helpers::test_key_sequences(
        &mut app,
        vec![
            // Let the asynchronous confirmation dialog open before accepting it.
            (Some("i# unsaved<esc>:config-migrate<ret>"), None),
            (
                Some("<down><ret>"),
                Some(&|app: &term::application::Application| {
                    assert!(view::current_ref!(app.editor).1.is_modified());
                    assert!(app
                        .editor
                        .get_status()
                        .unwrap()
                        .0
                        .contains("unsaved configuration changes"));
                    assert_eq!(std::fs::read_to_string(&user).unwrap(), original);
                    assert!(!user.with_extension("toml.bak").exists());
                }),
            ),
        ],
        false,
    )
    .await
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn config_reload_reads_read_only_symlinks_without_writes() -> anyhow::Result<()> {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let Some(root) = super::helpers::isolation::workspace(
        "test::config_application::config_reload_reads_read_only_symlinks_without_writes",
    )?
    else {
        return Ok(());
    };
    let store = root.join("managed");
    std::fs::create_dir(&store)?;
    let target = store.join("config.toml");
    let user = loader::config_file();
    let source = "[editor]\nscrolloff = 9\nfile-watcher.enable = false\n";
    std::fs::write(&target, source)?;
    symlink(&target, &user)?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444))?;
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o555))?;
    let loaded = term::config::Config::load_default().unwrap();
    let mut app = AppBuilder::new().with_config(loaded).build()?;
    app.handle_config_events(ConfigEvent::Refresh);
    let (message, severity) = app.editor.get_status().unwrap();
    assert_eq!(message, "Config refreshed");
    assert_eq!(*severity, editor_core::diagnostic::Severity::Info);
    assert_eq!(app.editor.config().scroll.padding, 9);
    assert_eq!(std::fs::read_to_string(&target)?, source);
    assert_eq!(std::fs::read_dir(&store)?.count(), 1);
    assert!(std::fs::symlink_metadata(&user)?.is_symlink());
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755))?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))?;
    assert!(app.close().await.is_empty());
    Ok(())
}
