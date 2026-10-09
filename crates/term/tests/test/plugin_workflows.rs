//! Public SDK workflows through native dialogs, storage, navigation and jobs.
use super::helpers::{run_event_loop_until_idle, test_config, AppBuilder};
use plugin_api::{Capability, Permissions, ProcessGrant};
use plugins::PluginConfig;
use std::{collections::BTreeSet, time::Duration};
use term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;
use ui_core::input::parse_macro;

fn package(root: &std::path::Path) -> anyhow::Result<(String, PluginConfig)> {
    let name = format!(
        "wf-{}",
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches('.')
    );
    let package = root.join("package");
    std::fs::create_dir(&package)?;
    std::fs::write(
        package.join("plugin.toml"),
        include_str!("../../../plugins/tests/fixtures/workflow-guest/plugin.toml"),
    )?;
    std::fs::write(
        package.join("workflows.component.wasm"),
        include_bytes!("../../../plugins/tests/fixtures/workflows.component.wasm"),
    )?;
    Ok((
        name,
        PluginConfig {
            path: package,
            config: serde_json::json!({"root-path":root}),
            permissions: Permissions {
                capabilities: BTreeSet::from([
                    Capability::Ui,
                    Capability::EditorRead,
                    Capability::EditorEdit,
                    Capability::EditorSelection,
                    Capability::EditorNavigate,
                    Capability::WorkspaceRead,
                    Capability::Storage,
                    Capability::Process,
                ]),
                read_roots: vec![root.into()],
                ..Default::default()
            },
            ..Default::default()
        },
    ))
}

async fn until(
    app: &mut Application,
    condition: impl Fn(&Application) -> bool,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(app).await;
            if condition(app) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

async fn keys(app: &mut Application, input: &str) -> anyhow::Result<()> {
    if app.editor.has_pending_plugin_work()
        || app.editor.documents().any(|doc| doc.is_syntax_pending())
    {
        app.editor.reset_idle_timer();
        tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(app)).await?;
    }
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro(input)? {
        #[cfg(not(windows))]
        let event = termina::Event::Key(termina::event::KeyEvent::from(key));
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(crossterm::event::KeyEvent::from(key));
        sender.send(Ok(event))?;
    }
    let mut stream = UnboundedReceiverStream::new(receiver);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(10),
            app.event_loop_until_idle(&mut stream)
        )
        .await?
    );
    Ok(())
}

fn dialog(app: &Application) -> bool {
    app.has_plugin_ui()
}

#[tokio::test(flavor = "multi_thread")]
async fn public_workflows_persist_recent_files_and_search_unicode_locations() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let first = root.path().join("first.txt");
    let second = root.path().join("second.txt");
    std::fs::write(&first, "α needle\n")?;
    std::fs::write(&second, "second\n")?;
    let first = std::fs::canonicalize(first)?;
    let second = std::fs::canonicalize(second)?;
    let (name, plugin) = package(root.path())?;
    let mut config = test_config();
    config.plugins.insert(name.clone(), plugin.clone());
    let mut app = AppBuilder::new().with_config(config).build()?;
    run_event_loop_until_idle(&mut app).await;
    app.editor.open(&first, view::editor::Action::Replace)?;
    run_event_loop_until_idle(&mut app).await;
    app.editor.open(&second, view::editor::Action::Replace)?;
    run_event_loop_until_idle(&mut app).await;
    assert!(app
        .editor
        .reload_plugins(&[(name.clone(), plugin)].into(), root.path()));
    run_event_loop_until_idle(&mut app).await;
    let recent = format!(":{name}.recent<ret>");
    keys(&mut app, &recent).await?;
    keys(&mut app, "<down><ret>").await?;
    assert_eq!(view::doc!(app.editor).path(), Some(first.as_path()));
    let search = format!(":{name}.search needle<ret>");
    keys(&mut app, &search).await?;
    keys(&mut app, "<ret>").await?;
    until(&mut app, dialog).await?;
    keys(&mut app, "<ret>").await?;
    let (view, doc) = view::current_ref!(app.editor);
    assert_eq!(doc.path(), Some(first.as_path()));
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        2
    );
    assert_eq!(app.editor.error_revision(), 0);
    let cancel_dialog = format!(":{name}.recent<ret>");
    keys(&mut app, &cancel_dialog).await?;
    keys(&mut app, "<esc>").await?;
    assert_eq!(
        app.editor.get_status().unwrap().0,
        "Workflow dialog cancelled"
    );
    assert!(!dialog(&app));
    let storage = loader::data_dir().join("plugins").join(name);
    app.editor.shutdown_plugins();
    tokio::time::timeout(Duration::from_secs(20), app.editor.finish_plugin_shutdown()).await?;
    let _ = std::fs::remove_dir_all(storage);
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn public_formatter_applies_revisioned_output_and_rejects_typing_races() -> anyhow::Result<()>
{
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir()?;
    let tool = root.path().join("formatter");
    std::fs::write(
        &tool,
        "#!/bin/sh\n/bin/cat >/dev/null\n/bin/sleep 2\nprintf 'FORMATTED\\n'\n",
    )?;
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))?;
    let (name, mut plugin) = package(root.path())?;
    plugin.config["formatter"] = serde_json::json!(tool);
    plugin.permissions.processes = vec![ProcessGrant {
        command: tool.to_string_lossy().into(),
        args: vec![
            "--emit".into(),
            "stdout".into(),
            "--edition".into(),
            "2024".into(),
        ],
    }];
    let mut config = test_config();
    config.plugins.insert(name.clone(), plugin);
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[a|]#bc\n")
        .build()?;
    let format = format!(":{name}.format<ret>");
    keys(&mut app, &format).await?;
    until(&mut app, |app| {
        view::doc!(app.editor).text().to_string() == "FORMATTED\n"
    })
    .await?;
    assert_eq!(
        app.editor.get_status().unwrap().0,
        "Formatting completed; save when ready"
    );
    keys(&mut app, "u").await?;
    keys(&mut app, &format).await?;
    keys(&mut app, "iμ<esc>").await?;
    until(&mut app, |app| app.editor.is_err()).await?;
    assert!(view::doc!(app.editor).text().to_string().contains('μ'));
    assert!(!view::doc!(app.editor)
        .text()
        .to_string()
        .contains("FORMATTED"));
    assert!(
        app.editor
            .get_status()
            .unwrap()
            .0
            .contains("stale document version"),
        "{:?}",
        app.editor.get_status()
    );
    let storage = loader::data_dir().join("plugins").join(name);
    app.editor.shutdown_plugins();
    tokio::time::timeout(Duration::from_secs(20), app.editor.finish_plugin_shutdown()).await?;
    let _ = std::fs::remove_dir_all(storage);
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn public_workflows_deny_tools_cancel_jobs_and_remove_dialogs_on_reload() -> anyhow::Result<()>
{
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir()?;
    let tool = root.path().join("formatter");
    std::fs::write(
        &tool,
        "#!/bin/sh\n/bin/cat >/dev/null\nexec /bin/sleep 30\n",
    )?;
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))?;
    let (name, mut plugin) = package(root.path())?;
    plugin.config["formatter"] = serde_json::json!(tool);
    let mut config = test_config();
    config.plugins.insert(name.clone(), plugin.clone());
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[a|]#bc\n")
        .build()?;
    let format = format!(":{name}.format<ret>");
    keys(&mut app, &format).await?;
    assert!(app.editor.is_err());
    assert!(
        app.editor.get_status().unwrap().0.contains("grant"),
        "{:?}",
        app.editor.get_status()
    );
    assert!(app
        .editor
        .plugin_command_doc(&format!("{name}.format"))
        .is_some());
    plugin.permissions.processes = vec![ProcessGrant {
        command: tool.to_string_lossy().into(),
        args: vec![
            "--emit".into(),
            "stdout".into(),
            "--edition".into(),
            "2024".into(),
        ],
    }];
    assert!(app
        .editor
        .reload_plugins(&[(name.clone(), plugin)].into(), root.path()));
    keys(&mut app, &format).await?;
    assert_eq!(
        app.editor.get_status().unwrap().0,
        "Formatting started; wait for completion before saving"
    );
    let cancel = format!(":{name}.cancel<ret>");
    let start = std::time::Instant::now();
    keys(&mut app, &cancel).await?;
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(app.editor.get_status().unwrap().0, "Workflow job cancelled");
    assert_eq!(view::doc!(app.editor).text().to_string(), "abc\n");
    keys(&mut app, &format!(":{name}.recent<ret>")).await?;
    assert!(dialog(&app));
    assert!(app.editor.reload_plugins(&Default::default(), root.path()));
    run_event_loop_until_idle(&mut app).await;
    assert!(!dialog(&app));
    assert!(app.editor.plugin_commands().is_empty());
    let storage = loader::data_dir().join("plugins").join(name);
    app.editor.shutdown_plugins();
    tokio::time::timeout(Duration::from_secs(20), app.editor.finish_plugin_shutdown()).await?;
    let _ = std::fs::remove_dir_all(storage);
    Ok(())
}
