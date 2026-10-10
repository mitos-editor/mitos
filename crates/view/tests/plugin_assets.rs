//! Declarative contributions prepare as one set, preserve native precedence and
//! restore resources without adopting a base language's provider authority.

#[allow(dead_code)]
mod support;

use std::{collections::BTreeMap, sync::Arc};

use plugin_api::assets::{
    AssetOwner, LanguageAsset, LanguageProfile, OwnedAssets, QueryKind, SnippetAsset, ThemeAsset,
};
use view::plugins::assets::AssetRegistry;

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
