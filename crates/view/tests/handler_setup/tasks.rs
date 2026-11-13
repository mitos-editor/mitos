use super::Fixture;
use anyhow::Context;
use editor_config::TaskConfig;
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use view::{
    editor::Action,
    features::tasks::TaskStatus,
    quicklist::{QuicklistPosition, QuicklistTarget},
};

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

fn configure(f: &mut Fixture, command: String) {
    let mut config = (**f.config.load()).clone();
    config.shell = vec!["sh".into(), "-c".into()];
    config.tasks.insert(
        "build".into(),
        TaskConfig {
            command,
            cwd: Some(f.dir.path().to_owned()),
            env: BTreeMap::from([("MITOS_TASK_FIXTURE".into(), "captured".into())]),
            error_pattern: None,
        },
    );
    f.config.store(Arc::new(config));
}

async fn until(f: &mut Fixture, ready: impl Fn(&view::Editor) -> bool) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready(&f.editor) {
            f.callbacks.recv().await.context("callback queue closed")?(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_while_editing_reopens_output_and_reruns_captured_task() -> anyhow::Result<()> {
    let mut f = Fixture::new("source\n")?;
    let source = view::current_ref!(f.editor).1.id();
    let release = f.dir.path().join("release");
    configure(&mut f, format!(
        "printf 'src/main.rs:2:3: error: broken 🦀\\n'; while [ ! -f {} ]; do sleep 0.01; done; printf '%s\\n' \"$MITOS_TASK_FIXTURE\"; exit 7", quoted(&release)
    ));
    let output = f.editor.start_task("build")?;
    until(&mut f, |editor| {
        editor
            .task_output_text()
            .unwrap_or_default()
            .contains("\nsrc/main.rs:2:3: error: broken 🦀\n")
    })
    .await?;
    assert!(f.editor.task_running());
    assert!(f.editor.start_task("build").is_err());
    assert!(f.editor.document(output).unwrap().is_content_readonly());
    assert_eq!(f.editor.task_errors()?, 1);
    let entry = &f.editor.quicklist.entries()[0];
    assert_eq!(
        entry.target,
        QuicklistTarget::Path(f.dir.path().join("src/main.rs"))
    );
    assert_eq!(
        entry.position,
        QuicklistPosition::LineColRange {
            start_line: 1,
            start_col: 2,
            end_line: 1,
            end_col: 2
        }
    );

    f.editor.switch(source, Action::Replace);
    f.replace("edited while building\n");
    assert!(f.editor.close_document(output, false).is_ok());
    std::fs::write(&release, "ready")?;
    until(&mut f, |editor| !editor.task_running()).await?;
    assert!(matches!(
        f.editor.task_status(),
        Some(TaskStatus::Failed(_))
    ));
    assert!(f.editor.task_output_text().unwrap().contains("captured\n"));
    assert!(f.editor.document(output).is_none());
    let reopened = f.editor.open_task_output(Action::Replace)?;
    assert_eq!(
        f.editor.document(reopened).unwrap().text().to_string(),
        f.editor.task_output_text().unwrap()
    );
    assert_eq!(
        f.editor.document(source).unwrap().text().to_string(),
        "edited while building\n"
    );

    configure(&mut f, "printf 'changed command\\n'".into());
    f.editor.rerun_task()?;
    until(&mut f, |editor| !editor.task_running()).await?;
    assert!(matches!(
        f.editor.task_status(),
        Some(TaskStatus::Failed(_))
    ));
    assert!(f.editor.task_output_text().unwrap().contains("captured\n"));
    assert!(!f
        .editor
        .task_output_text()
        .unwrap()
        .contains("changed command"));
    assert_eq!(f.editor.quicklist.entries().len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_finishes_and_subsequent_success_clears_old_locations() -> anyhow::Result<()> {
    let mut f = Fixture::new("source\n")?;
    configure(&mut f, "printf 'source.rs:1:1: error\\n'; sleep 60".into());
    f.editor.start_task("build")?;
    until(&mut f, |editor| {
        editor
            .task_output_text()
            .unwrap_or_default()
            .contains("error\n")
    })
    .await?;
    f.editor.cancel_task()?;
    until(&mut f, |editor| !editor.task_running()).await?;
    assert_eq!(f.editor.task_status(), Some(&TaskStatus::Cancelled));
    assert!(f.editor.task_output_text().unwrap().contains("cancelled"));
    assert_eq!(f.editor.quicklist.entries().len(), 1);
    configure(&mut f, "printf 'all tests passed\\n'".into());
    f.editor.start_task("build")?;
    until(&mut f, |editor| !editor.task_running()).await?;
    assert_eq!(f.editor.task_status(), Some(&TaskStatus::Succeeded));
    assert!(f.editor.quicklist.entries().is_empty());
    Ok(())
}

#[tokio::test]
async fn refuses_untrusted_and_invalid_tasks_without_starting_a_process() -> anyhow::Result<()> {
    let mut f = Fixture::new("source\n")?;
    configure(&mut f, "echo must-not-run".into());
    f.editor.workspace_trust = workspace::trust::WorkspaceTrust::new(
        workspace::trust::Config::default(),
        f.dir.path().join("untrusted.toml"),
    );
    assert!(f.editor.start_task("build").is_err());
    assert!(!f.editor.task_running());
    f.editor.workspace_trust =
        workspace::trust::WorkspaceTrust::fully_trusted(f.dir.path().join("trusted.toml"));
    let mut config = (**f.config.load()).clone();
    config.tasks.get_mut("build").unwrap().error_pattern = Some("[".into());
    f.config.store(Arc::new(config));
    assert!(f.editor.start_task("build").is_err());
    assert!(f.editor.task_output_text().is_none());
    Ok(())
}
