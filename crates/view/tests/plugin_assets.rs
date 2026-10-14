//! Declarative contributions prepare as one set, preserve native precedence and
//! restore resources without adopting a base language's provider authority.

#[allow(dead_code)]
mod support;

use std::{collections::BTreeMap, sync::Arc};

use plugin_api::assets::{
    AssetOwner, LanguageAsset, LanguageProfile, OwnedAssets, QueryKind, SnippetAsset, ThemeAsset,
};
use view::plugins::assets::AssetRegistry;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declarative_package_provides_native_theme_without_guest_execution() -> anyhow::Result<()> {
    use plugin_api::diagnostics::{PluginEngine, PluginStatus};
    use plugins::{PluginConfig, PluginManager};
    use view::theme::Color;

    let mut fixture = support::Fixture::new("Theme package\n")?;
    let native_name = fixture.editor.theme.name().to_owned();
    assert!(fixture.editor.theme_loader.load("fixture.dark").is_err());
    let package = tempfile::tempdir()?;
    let root = package.path();
    std::fs::create_dir(root.join("themes"))?;
    std::fs::write(
        root.join("plugin.toml"),
        r#"
manifest-version = 1
api-version = "0.1.0"
minimum-host-version = "0.1.0"
abi-version = 3
capabilities = []
[[contributions.themes]]
name = "dark"
path = "themes/dark.toml"
"#,
    )?;
    std::fs::write(
        root.join("themes/dark.toml"),
        r##"
"ui.background" = { bg = "#16181a" }
"ui.text" = "#ffffff"
"ui.selection" = { bg = "#3c4048" }
"ui.selection.primary" = { bg = "#3c4048" }
"##,
    )?;
    let config = PluginConfig {
        path: root.join("plugin.toml"),
        permissions: plugin_api::Permissions {
            capabilities: Default::default(),
            ..Default::default()
        },
        ..Default::default()
    };
    let prepared = PluginManager::default()
        .prepare([("fixture".into(), config)].into(), root.to_owned(), 1)?
        .await?;
    let mut registry = AssetRegistry::default();
    let assets = registry.capture(&fixture.editor, prepared.assets());
    let assets = tokio::task::spawn_blocking(move || assets.prepare()).await??;
    assets.activate(&mut registry, &mut fixture.editor)?;
    let manager = prepared.activate()?;
    assert!(manager.available_commands().is_empty());
    let report = manager.diagnostics();
    assert_eq!(report.len(), 1);
    assert!(matches!(report[0].engine, Some(PluginEngine::Declarative)));
    assert!(matches!(report[0].status, PluginStatus::Ready));
    assert!(report[0].declared.is_empty() && report[0].effective.is_empty());
    assert_eq!(report[0].timings.completed, 0);
    let (theme, warnings) = fixture
        .editor
        .theme_loader
        .load_with_warnings("fixture.dark")?;
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(theme.get("ui.background").bg, Some(Color::Rgb(22, 24, 26)));
    assert_eq!(theme.get("ui.text").fg, Some(Color::Rgb(255, 255, 255)));
    assert_eq!(
        theme.get("ui.selection.primary").bg,
        Some(Color::Rgb(60, 64, 72))
    );
    fixture.editor.set_theme(theme)?;
    registry.restore(&mut fixture.editor);
    assert_eq!(fixture.editor.theme.name(), native_name);
    assert!(fixture.editor.theme_loader.load("fixture.dark").is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_assets_validate_before_swap_and_restore_native_sources() -> anyhow::Result<()> {
    let mut fixture = support::Fixture::with_languages(
        "{\"key\":1}\n",
        r#"
        [[language]]
        name = "json"
        grammar = "json"
        scope = "source.json"
        file-types = ["json"]
        auto-format = true
        formatter = { command = "forbidden-formatter" }
        language-servers = ["forbidden-server"]
    "#,
    )?;
    let mut registry = AssetRegistry::default();
    let original_syntax = fixture.editor.syn_loader.load_full();
    let original_themes = fixture.editor.theme_loader.clone();
    let empty = registry.capture(&fixture.editor, vec![]);
    let prepared = tokio::task::spawn_blocking(move || empty.prepare()).await??;
    prepared.activate(&mut registry, &mut fixture.editor)?;
    assert!(Arc::ptr_eq(
        &original_syntax,
        &fixture.editor.syn_loader.load_full()
    ));
    assert!(Arc::ptr_eq(&original_themes, &fixture.editor.theme_loader));
    let mut package = OwnedAssets {
        owner: AssetOwner {
            plugin: "fixture".into(),
            generation: 1,
        },
        themes: vec![ThemeAsset {
            name: "tone".into(),
            source: "inherits = 'default'\n\"ui.text\" = 'red'".into(),
        }],
        languages: vec![LanguageAsset {
            name: "data".into(),
            profile: LanguageProfile {
                base_language: "json".into(),
                scope: "source.fixture".into(),
                extensions: vec!["words".into(), "json".into()],
                queries: BTreeMap::new(),
            },
            queries: [(QueryKind::Highlights, "(string) @string".into())].into(),
        }],
        snippets: vec![SnippetAsset {
            language: "fixture.data".into(),
            prefix: "obj".into(),
            body: "{\"${1:key}\":${2:value}}$0".into(),
            description: "Object".into(),
        }],
    };
    let captured = registry.capture(&fixture.editor, vec![Arc::new(package.clone())]);
    let prepared = tokio::task::spawn_blocking(move || captured.prepare()).await??;
    prepared.activate(&mut registry, &mut fixture.editor)?;
    assert_eq!(registry.snippets().len(), 1);
    assert!(fixture.editor.theme_loader.load("fixture.tone").is_ok());
    let active = fixture.editor.syn_loader.load_full();
    let native = active
        .language_for_filename(std::path::Path::new("file.json"))
        .unwrap();
    assert_eq!(active.language(native).config().language_id, "json");
    let doc = view::doc!(fixture.editor);
    assert_eq!(doc.language_name(), Some("fixture.data"));
    let config = doc.language_config().unwrap();
    assert!(
        config.formatter.is_none()
            && config.language_servers.is_empty()
            && config.debugger.is_none()
    );
    assert_eq!(config.auto_format, Some(false));

    for source in [
        "(missing_node) @string".to_owned(),
        // Valid native syntax, but the whole-document local reference would
        // materialize captured text during highlighting.
        "(document) @local.reference".to_owned(),
        "(".repeat(65),
        "((string) @string (#match? @string \"a{99999999}\"))".to_owned(),
    ] {
        package.languages[0]
            .queries
            .insert(QueryKind::Highlights, source);
        let captured = registry.capture(&fixture.editor, vec![Arc::new(package.clone())]);
        assert!(tokio::task::spawn_blocking(move || captured.prepare())
            .await?
            .is_err());
        assert!(Arc::ptr_eq(&active, &fixture.editor.syn_loader.load_full()));
        assert!(fixture.editor.theme_loader.load("fixture.tone").is_ok());
    }

    let captured = registry.capture(&fixture.editor, vec![]);
    let prepared = tokio::task::spawn_blocking(move || captured.prepare()).await??;
    fixture.editor.theme_loader = Arc::new(view::theme::Loader::new(
        loader::theme::Resources::new(vec![fixture.dir.path().to_owned()]),
    ));
    assert!(!prepared.is_current(&fixture.editor));
    assert!(prepared
        .activate(&mut registry, &mut fixture.editor)
        .is_err());
    assert!(Arc::ptr_eq(&active, &fixture.editor.syn_loader.load_full()));

    let captured = registry.capture(&fixture.editor, vec![]);
    let prepared = tokio::task::spawn_blocking(move || captured.prepare()).await??;
    prepared.activate(&mut registry, &mut fixture.editor)?;
    assert!(registry.snippets().is_empty());
    assert!(fixture.editor.theme_loader.load("fixture.tone").is_err());
    assert!(fixture
        .editor
        .syn_loader
        .load()
        .language_for_name("fixture.data")
        .is_none());
    // The .words document had no native language; unloading cannot transfer the
    // approved JSON base's formatter/server authority to that unrelated suffix.
    assert!(view::doc!(fixture.editor).language_config().is_none());

    package.languages[0]
        .queries
        .insert(QueryKind::Highlights, "(string) @string".into());
    let native_syntax = fixture.editor.syn_loader.load_full();
    let native_theme_name = fixture.editor.theme.name().to_owned();
    let captured = registry.capture(&fixture.editor, vec![Arc::new(package)]);
    let prepared = tokio::task::spawn_blocking(move || captured.prepare()).await??;
    prepared.activate(&mut registry, &mut fixture.editor)?;
    let owned = fixture.editor.theme_loader.load("fixture.tone")?;
    fixture.editor.set_theme(owned)?;
    registry.restore(&mut fixture.editor);
    assert!(Arc::ptr_eq(
        &native_syntax,
        &fixture.editor.syn_loader.load_full()
    ));
    assert_eq!(fixture.editor.theme.name(), native_theme_name);
    assert!(registry.snippets().is_empty());
    assert!(registry.owners().next().is_none());
    assert!(fixture.editor.theme_loader.load("fixture.tone").is_err());
    assert!(view::doc!(fixture.editor).language_config().is_none());
    Ok(())
}
