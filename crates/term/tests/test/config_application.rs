use std::{
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use editor_core::{syntax::config::AutoPairConfig, Selection};
use loader::workspace_trust::{ImplicitTrustLevel, TrustQuery};
use serde_json::json;
use view::{
    config::ImplicitTrustLevelConfig,
    current_ref,
    editor::{Action, ConfigEvent},
    events::ConfigDidChange,
    theme::Theme,
    view::ViewPosition,
};

use super::helpers::{lsp::Fixture, test_syntax_loader, AppBuilder};

#[tokio::test(flavor = "multi_thread")]
async fn live_settings_are_visible_to_hooks_and_all_views_are_adjusted_afterward(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("document.txt");
    std::fs::write(&path, "line\n".repeat(500))?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    app.editor.open(&path, Action::VerticalSplit)?;
    assert_eq!(app.editor.tree.views().count(), 2);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    event::register_hook!(move |event: &mut ConfigDidChange<'_>| {
        assert!(!event.old.soft_wrap.enable.unwrap_or(false));
        assert_eq!(event.new.soft_wrap.enable, Some(true));
        assert_eq!(event.editor.config().soft_wrap.enable, Some(true));
        assert!(event
            .editor
            .auto_pairs
            .as_ref()
            .is_none_or(|pairs| pairs.get('(').is_none()));
        // A hook changes selection after the editor's initial layout refresh.
        // The final cursor adjustment must use this selection in every split.
        for (view, _) in event.editor.tree.views() {
            let doc = event.editor.documents.get_mut(&view.doc).unwrap();
            doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 2));
            doc.set_view_offset(view.id, ViewPosition::default());
        }
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let mut settings = (*app.editor.config()).clone();
    settings.soft_wrap.enable = Some(true);
    settings.auto_pairs = AutoPairConfig::Enable(false);
    settings.scrolloff = 3;
    app.handle_config_events(ConfigEvent::Update(Box::new(settings)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for (view, _) in app.editor.tree.views_mut() {
        let doc = app.editor.documents.get(&view.doc).unwrap();
        assert!(doc.view_offset(view.id).anchor > 0);
        assert!(view.is_cursor_in_view(doc, 3));
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

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
async fn invalid_theme_keeps_the_previous_theme_and_still_refreshes_documents() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("document.new-config-test");
    std::fs::write(&path, "text\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let old_theme = app.editor.theme.scopes().to_vec();
    let loader = test_syntax_loader(Some(
        r#"
        [[language]]
        name = "new-config-test"
        scope = "source.new-config-test"
        file-types = ["new-config-test"]
        roots = []
    "#
        .into(),
    ));
    let invalid_theme = Theme::from(toml::Value::Table(Default::default()));
    let error = app
        .editor
        .apply_language_config(loader, invalid_theme)
        .unwrap_err();
    assert!(error.to_string().contains("ui.selection"));
    assert_eq!(app.editor.theme.scopes(), old_theme);
    assert_eq!(
        current_ref!(app.editor)
            .1
            .language_config()
            .unwrap()
            .language_id,
        "new-config-test"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

/// File lookup and the config-file OnceLock are process-wide. Exercise real
/// discovery in a child with isolated directories, leaving parallel tests alone.
fn isolated_workspace(name: &str) -> anyhow::Result<Option<PathBuf>> {
    const ROOT: &str = "MITOS_TEST_CONFIG_APPLICATION_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = PathBuf::from(root);
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
            &format!("test::config_application::{name}"),
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

#[tokio::test(flavor = "multi_thread")]
async fn language_loading_applies_new_trust_before_reading_local_files() -> anyhow::Result<()> {
    let Some(root) =
        isolated_workspace("language_loading_applies_new_trust_before_reading_local_files")?
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
    let Some(root) = isolated_workspace(
        "application_reload_publishes_settings_after_resources_and_preserves_failure_boundaries",
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
