use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use loader::workspace_trust::{ImplicitTrustLevel, TrustQuery};
use serde_json::json;
use view::{
    config::ImplicitTrustLevelConfig,
    current_ref,
    editor::{Action, ConfigEvent},
    events::ConfigDidChange,
    theme::Theme,
};

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
        assert_eq!(event.new.scrolloff, 7);
        assert_eq!(event.editor.config().scrolloff, 7);
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
            .replace("scrolloff = 7", "scrolloff = 9"),
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
    assert_eq!(app.editor.config().scrolloff, 7);
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
