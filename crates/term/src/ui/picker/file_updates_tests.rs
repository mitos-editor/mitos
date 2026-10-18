use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use super::super::*;
use crate::{application::Application, config::Config as AppConfig, job::Jobs};
use view::file_watcher::{CanonicalPathBuf, Event as FileEvent, EventType, Events};

fn application(poll: bool) -> anyhow::Result<Application> {
    let mut config = AppConfig::default();
    config.editor.lsp.enable = false;
    config.editor.files.watch.enable = false;
    config.editor.files.watch.watch_vcs = false;
    config.editor.files.reload.enable = false;
    config.editor.files.reload.poll.enable = poll;
    config.editor.files.reload.poll.interval = 100;
    Application::new(
        Default::default(),
        config,
        language::syntax::Loader::new(
            language_config::Configuration {
                language: vec![],
                language_server: Default::default(),
            },
            Default::default(),
        )?,
        workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
    )
}

fn draw<T: Send + Sync, D: Send + Sync>(picker: &mut Picker<T, D>, app: &mut Application) {
    let config = AppConfig::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = Jobs::new();
    let mut cx = Context {
        config: crate::config::Context {
            current: &config,
            updates: &updates,
        },
        editor: &mut app.editor,
        jobs: &mut jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    let area = Rect::new(0, 0, 70, 20);
    picker.render(area, &mut Surface::empty(area), &mut cx);
}

async fn until<T: Send + Sync, D: Send + Sync>(
    picker: &mut Picker<T, D>,
    app: &mut Application,
    ready: impl Fn(&Picker<T, D>) -> bool,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            draw(picker, app);
            if !picker.refreshing && ready(picker) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

fn changed(editor: &mut Editor, path: &Path, ty: EventType) {
    editor.handle_file_events(&Events::from(vec![FileEvent {
        path: CanonicalPathBuf::assert_canonicalized(&stdx::path::canonicalize_existing(path)),
        ty,
    }]));
}

#[tokio::test]
async fn continuous_changes_publish_completed_scans_and_queue_a_followup() -> anyhow::Result<()> {
    type Scan = tokio::sync::oneshot::Sender<anyhow::Result<Vec<PathBuf>>>;
    struct Data {
        root: PathBuf,
        started: tokio::sync::mpsc::UnboundedSender<Scan>,
    }
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    let (started, mut scans) = tokio::sync::mpsc::unbounded_channel();
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("path", |path: &PathBuf, _: &Data| {
            path.to_string_lossy().into_owned().into()
        })],
        0,
        [dir.path().join("old.txt")],
        Data {
            root: dir.path().to_owned(),
            started,
        },
        |_, _, _| {},
    )
    .with_file_updates(
        &app.editor,
        |data| &data.root,
        |_, _, data, _| {
            Box::pin(async move {
                let (tx, rx) = tokio::sync::oneshot::channel();
                data.started.send(tx).unwrap();
                rx.await?
            })
        },
        |path| path,
    );
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 1
    })
    .await?;
    changed(
        &mut app.editor,
        &dir.path().join("new.txt"),
        EventType::Create,
    );
    draw(&mut picker, &mut app);
    for index in 0..3 {
        let complete = tokio::time::timeout(Duration::from_secs(5), scans.recv())
            .await?
            .unwrap();
        let latest = dir.path().join(format!("latest-{index}.txt"));
        changed(&mut app.editor, &latest, EventType::Create);
        complete.send(Ok(vec![latest.clone()])).unwrap();
        until(&mut picker, &mut app, |picker| {
            picker.selection() == Some(&latest)
                && picker.file_updates.as_ref().unwrap().pending.is_some()
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn file_refresh_keeps_the_query_and_selected_path() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_owned();
    std::fs::write(root.join("b-keep.txt"), "b")?;
    std::fs::write(root.join("z-keep.txt"), "z")?;
    let mut picker = ui::file_picker(&app.editor, root.clone());
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 2
    })
    .await?;
    picker.prompt.set_line("keep".into(), &app.editor);
    picker.handle_prompt_change(false);
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().matched_item_count() == 2
    })
    .await?;
    picker.cursor = 1;
    let selected = picker.selection().unwrap().clone();
    std::fs::write(root.join("a-keep.txt"), "a")?;
    changed(&mut app.editor, &root.join("a-keep.txt"), EventType::Create);
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 3
    })
    .await?;
    let version = picker.version.load(atomic::Ordering::Relaxed);
    changed(&mut app.editor, &root.join("a-keep.txt"), EventType::Create);
    until(&mut picker, &mut app, |picker| {
        picker.file_updates.as_ref().unwrap().pending.is_none()
    })
    .await?;
    assert_eq!(picker.version.load(atomic::Ordering::Relaxed), version);
    assert_eq!(picker.primary_query().as_ref(), "keep");
    assert_eq!(picker.selection(), Some(&selected));
    assert_eq!(picker.cursor, 2);
    std::fs::remove_file(&selected)?;
    changed(&mut app.editor, &selected, EventType::Delete);
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 2
    })
    .await?;
    assert!(picker.selection().is_some_and(|path| path != &selected));
    Ok(())
}

#[tokio::test]
async fn explorer_polling_covers_hidden_ignored_files_and_external_renames() -> anyhow::Result<()> {
    let mut app = application(true)?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
    let mut picker = ui::file_explorer(dir.path().to_owned(), &app.editor)?;
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 2
    })
    .await?;
    let hidden = dir.path().join(".hidden.txt");
    let ignored = dir.path().join("ignored.txt");
    std::fs::write(&hidden, "hidden")?;
    std::fs::write(&ignored, "ignored")?;
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 4
    })
    .await?;
    let renamed = dir.path().join("renamed.txt");
    std::fs::rename(&hidden, &renamed)?;
    until(&mut picker, &mut app, |picker| {
        picker
            .matcher
            .snapshot()
            .matched_items(0..picker.matcher.snapshot().matched_item_count())
            .any(|item| item.data.0 == renamed)
    })
    .await?;
    std::fs::remove_file(&ignored)?;
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 3
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn stopping_a_refresh_cancels_its_worker_and_rechecks_on_resume() -> anyhow::Result<()> {
    struct Data {
        root: PathBuf,
        started: tokio::sync::mpsc::UnboundedSender<event::TaskHandle>,
    }
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    let (started, mut workers) = tokio::sync::mpsc::unbounded_channel();
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("path", |path: &PathBuf, _: &Data| {
            path.to_string_lossy().into_owned().into()
        })],
        0,
        [dir.path().join("old")],
        Data {
            root: dir.path().to_owned(),
            started,
        },
        |_, _, _| {},
    )
    .with_file_updates(
        &app.editor,
        |data| &data.root,
        |_, _, data, cancel| {
            Box::pin(async move {
                data.started.send(cancel)?;
                std::future::pending().await
            })
        },
        |path| path,
    );
    changed(&mut app.editor, &dir.path().join("new"), EventType::Create);
    until(&mut picker, &mut app, |picker| {
        picker.file_updates.as_ref().unwrap().pending.is_some()
    })
    .await?;
    let worker = tokio::time::timeout(Duration::from_secs(5), workers.recv())
        .await?
        .unwrap();
    let started = picker.editor_data.started.clone();
    picker.reset_options(
        vec![dir.path().join("other/old")],
        Data {
            root: dir.path().join("other"),
            started,
        },
    );
    assert!(worker.is_canceled());
    draw(&mut picker, &mut app);
    let changed_root = tokio::time::timeout(Duration::from_secs(5), workers.recv())
        .await?
        .unwrap();
    assert!(!changed_root.is_canceled());
    picker.version.fetch_add(1, atomic::Ordering::Relaxed);
    draw(&mut picker, &mut app);
    assert!(changed_root.is_canceled());
    let current_query = tokio::time::timeout(Duration::from_secs(5), workers.recv())
        .await?
        .unwrap();
    picker.file_updates.as_mut().unwrap().stop();
    assert!(current_query.is_canceled());
    draw(&mut picker, &mut app);
    let resumed = tokio::time::timeout(Duration::from_secs(5), workers.recv())
        .await?
        .unwrap();
    assert!(!resumed.is_canceled());
    drop(picker);
    assert!(resumed.is_canceled());
    Ok(())
}

#[tokio::test]
async fn content_refresh_preserves_the_selected_location_within_a_file() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("same-file.txt");
    std::fs::write(&path, "before")?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new(
            "path",
            |item: &(PathBuf, usize), _: &PathBuf| item.0.to_string_lossy().into_owned().into(),
        )],
        0,
        [(path.clone(), 0), (path.clone(), 1)],
        dir.path().to_owned(),
        |_, _, _| {},
    )
    .with_file_updates(
        &app.editor,
        |root| root,
        |_, _, root, _| {
            Box::pin(async move {
                Ok(vec![
                    (root.join("another-file.txt"), 0),
                    (root.join("same-file.txt"), 0),
                    (root.join("same-file.txt"), 1),
                ])
            })
        },
        |item| &item.0,
    )
    .refresh_on_content_changes();
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 2
    })
    .await?;
    picker.cursor = 1;
    changed(&mut app.editor, &path, EventType::Modified);
    until(&mut picker, &mut app, |picker| {
        picker.matcher.snapshot().item_count() == 3
    })
    .await?;
    assert_eq!(picker.selection(), Some(&(path, 1)));
    assert_eq!(picker.cursor, 2);
    Ok(())
}
