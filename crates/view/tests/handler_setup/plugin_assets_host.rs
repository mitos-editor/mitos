//! Package discovery, worker preflight and editor activation are one replacement.
use super::Fixture;
use std::{collections::BTreeMap, sync::Arc};

async fn drain(fixture: &mut Fixture) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            fixture.editor.poll_plugin_events();
            while let Ok(callback) = fixture.callbacks.try_recv() {
                callback(&mut fixture.editor);
            }
            if !fixture.editor.has_pending_plugin_work() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("package preparation did not settle");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_contribution_keeps_active_code_and_assets_then_unload_restores_native_sources(
) -> anyhow::Result<()> {
    use crate::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::with_languages(
        "{}\n",
        r#"
        [[language]]
        name="json"
        scope="source.json"
        grammar="json"
        file-types=["json"]
    "#,
    )?;
    let dir = tempfile::tempdir()?;
    let config = observing(
        dir.path(),
        &[],
        &[Route {
            event: "command",
            response: status("active generation"),
            ..Route::default()
        }],
        None,
    )?;
    std::fs::write(
        dir.path().join("theme.toml"),
        "inherits='default'\n\"ui.text\"='red'\n",
    )?;
    std::fs::write(
        dir.path().join("snippets.toml"),
        "[[snippets]]\nprefix='obj'\nbody='{\"${1:key}\":${2:value}}$0'\ndescription='Object'\n",
    )?;
    let manifest = dir.path().join("plugin.toml");
    let mut data: toml::Value = toml::from_str(&std::fs::read_to_string(&manifest)?)?;
    data.as_table_mut().unwrap().insert(
        "contributions".into(),
        toml::Value::try_from(serde_json::json!({
            "themes":[{"name":"tone","path":"theme.toml"}],
            "snippets":[{"language":"json","path":"snippets.toml"}]
        }))?,
    );
    std::fs::write(&manifest, toml::to_string(&data)?)?;
    let packages = BTreeMap::from([("fixture".into(), config)]);
    assert!(fixture.editor.reload_plugins(&packages, dir.path()));
    drain(&mut fixture).await;
    assert_eq!(fixture.editor.plugin_snippets().len(), 1);
    assert!(fixture.editor.theme_loader.load("fixture.tone").is_ok());
    let active = fixture.editor.syn_loader.load_full();
    let active_themes = fixture.editor.theme_loader.clone();
    let errors = fixture.editor.error_revision();
    std::fs::write(
        dir.path().join("theme.toml"),
        "inherits='missing-native-theme'\n",
    )?;
    assert!(fixture.editor.reload_plugins(&packages, dir.path()));
    drain(&mut fixture).await;
    assert!(fixture.editor.error_revision() > errors);
    assert!(Arc::ptr_eq(&active, &fixture.editor.syn_loader.load_full()));
    assert!(Arc::ptr_eq(&active_themes, &fixture.editor.theme_loader));
    assert_eq!(fixture.editor.plugin_snippets().len(), 1);
    assert!(fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?);
    drain(&mut fixture).await;
    assert_eq!(fixture.editor.get_status().unwrap().0, "active generation");
    assert!(fixture.editor.reload_plugins(&BTreeMap::new(), dir.path()));
    drain(&mut fixture).await;
    assert!(fixture.editor.plugin_snippets().is_empty());
    assert!(fixture.editor.theme_loader.load("fixture.tone").is_err());
    assert!(fixture
        .editor
        .syn_loader
        .load()
        .language_for_name("json")
        .is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_theme_loader_change_while_shutdown_drains_rebases_before_publication(
) -> anyhow::Result<()> {
    use crate::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let mut old = observing(dir.path(), &["document-opened"], &[], None)?;
    old.config =
        serde_json::json!({"routes":[{"event":"shutdown","operation":"loop","once":true}]});
    assert!(fixture
        .editor
        .reload_plugins(&BTreeMap::from([("fixture".into(), old)]), dir.path()));
    drain(&mut fixture).await;
    assert!(fixture
        .editor
        .plugin_event_interested(plugin_api::Event::DocumentOpened));
    let replacement = observing(
        dir.path(),
        &["document-opened"],
        &[Route {
            event: "command",
            response: status("new generation"),
            ..Route::default()
        }],
        None,
    )?;
    std::fs::write(
        dir.path().join("theme.toml"),
        "inherits='default'\n\"ui.text\"='red'\n",
    )?;
    let path = dir.path().join("plugin.toml");
    let mut manifest: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
    manifest.as_table_mut().unwrap().insert(
        "contributions".into(),
        toml::Value::try_from(serde_json::json!({"themes":[{"name":"tone","path":"theme.toml"}]}))?,
    );
    std::fs::write(path, toml::to_string(&manifest)?)?;
    assert!(fixture.editor.reload_plugins(
        &BTreeMap::from([("fixture".into(), replacement)]),
        dir.path()
    ));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while fixture
            .editor
            .plugin_event_interested(plugin_api::Event::DocumentOpened)
        {
            fixture.editor.poll_plugin_events();
            while let Ok(callback) = fixture.callbacks.try_recv() {
                callback(&mut fixture.editor);
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("old generation never began retiring");
    let native = tempfile::tempdir()?;
    std::fs::create_dir(native.path().join("themes"))?;
    std::fs::write(
        native.path().join("themes/new-native.toml"),
        "inherits='default'\n\"ui.text\"='green'\n",
    )?;
    fixture.editor.theme_loader = Arc::new(view::theme::Loader::new(
        loader::theme::Resources::new(vec![native.path().to_owned()]),
    ));
    drain(&mut fixture).await;
    assert!(fixture.editor.theme_loader.load("new-native").is_ok());
    assert!(fixture.editor.theme_loader.load("fixture.tone").is_ok());
    assert!(fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?);
    drain(&mut fixture).await;
    assert_eq!(fixture.editor.get_status().unwrap().0, "new generation");
    fixture.editor.shutdown_plugins();
    fixture.editor.finish_plugin_shutdown().await;
    assert!(fixture.editor.theme_loader.load("fixture.tone").is_err());
    assert!(fixture.editor.theme_loader.load("new-native").is_ok());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_scoped_open_failure_is_counted_once_as_host_failure() -> anyhow::Result<()> {
    use crate::support::plugin_guest::{observing, Route};
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("image.bin");
    std::fs::write(&path, [0, 1, 0, 2, 0, 3])?;
    let config = observing(
        dir.path(),
        &[],
        &[Route {
            event: "command",
            response: plugin_api::Response {
                actions: vec![plugin_api::Action::Open {
                    path: path.to_string_lossy().into_owned(),
                }],
                error: None,
            },
            ..Route::default()
        }],
        None,
    )?;
    assert!(fixture
        .editor
        .reload_plugins(&BTreeMap::from([("fixture".into(), config)]), dir.path()));
    drain(&mut fixture).await;
    let before = fixture
        .editor
        .plugin_diagnostics()
        .into_iter()
        .find(|entry| entry.plugin == "fixture")
        .unwrap()
        .timings;
    let errors = fixture.editor.error_revision();
    assert!(fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?);
    drain(&mut fixture).await;
    assert!(fixture.editor.error_revision() > errors);
    let after = fixture
        .editor
        .plugin_diagnostics()
        .into_iter()
        .find(|entry| entry.plugin == "fixture")
        .unwrap()
        .timings;
    assert_eq!(after.failed, before.failed + 1);
    assert_eq!(after.completed, before.completed);
    assert_eq!(
        view::current_ref!(fixture.editor).1.text().to_string(),
        "original\n"
    );
    Ok(())
}
