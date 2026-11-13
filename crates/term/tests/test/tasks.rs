use super::helpers::{keys, run_event_loop_until_idle, screen, test_config, AppBuilder};
use std::time::Duration;
use term::{
    application::TestApplication,
    config::{Config, ConfigLoadError},
};
use view::{current_ref, features::tasks::TaskStatus};

async fn finished(app: &mut TestApplication) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.editor.task_running() {
            run_event_loop_until_idle(app).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn task_picker_output_search_quicklist_edit_and_rerun() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("failure.rs");
    std::fs::write(&source, "first\nsecond\n")?;
    let mut config = test_config();
    let source_config = format!(
        "[editor.tasks.build]\ncommand = 'echo failure.rs:2:3: fixture failure'\ncwd = {}\n",
        toml::Value::String(directory.path().to_string_lossy().into_owned())
    );
    config.editor.tasks = Config::load(Ok(&source_config), Err(ConfigLoadError::default()))
        .unwrap()
        .editor
        .tasks;
    let mut app = AppBuilder::new().with_config(config).build()?;
    keys(&mut app, ":task<ret>").await?;
    assert!(screen(&app).contains("Workspace tasks"));
    keys(&mut app, "build<ret>").await?;
    finished(&mut app).await?;
    assert_eq!(app.editor.task_status(), Some(&TaskStatus::Succeeded));
    let output = current_ref!(app.editor).1.id();
    assert!(app.editor.document(output).unwrap().is_content_readonly());
    keys(&mut app, "gg/fixture<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert!(doc
        .text()
        .slice(doc.selection(view.id).primary().from()..doc.selection(view.id).primary().to())
        .to_string()
        .contains("fixture"));
    keys(&mut app, "]q").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path(),
        Some(stdx::path::canonicalize(&source).as_path())
    );
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        8
    );
    keys(&mut app, "A!<esc>:write<ret>").await?;
    assert!(std::fs::read_to_string(&source)?.contains("second!"));
    keys(&mut app, ":task-output<ret>").await?;
    let views = app.editor.tree.views().count();
    for _ in 0..2 {
        keys(&mut app, ":task-rerun<ret>").await?;
        finished(&mut app).await?;
        assert_eq!(app.editor.tree.views().count(), views);
        assert_eq!(app.editor.quicklist.entries().len(), 1);
    }
    assert!(app.close().await.is_empty());
    Ok(())
}
