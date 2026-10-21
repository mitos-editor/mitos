use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use super::super::test::{application, draw, until};
use super::super::*;
use view::file_watcher::{CanonicalPathBuf, Event as FileEvent, EventType, Events};

fn changed(editor: &mut Editor, path: &Path, ty: EventType) {
    editor.handle_file_events(&Events::from(vec![FileEvent {
        path: CanonicalPathBuf::assert_canonicalized(&stdx::path::canonicalize_existing(path)),
        ty,
    }]));
}

#[tokio::test]
async fn refresh_keeps_navigation_and_viewport_until_publication() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    let paths: Vec<_> = (0..40)
        .map(|index| dir.path().join(format!("{index:02}.txt")))
        .collect();
    for path in &paths {
        std::fs::write(path, "preview")?;
    }
    let mut picker = ui::file_picker(&app.editor, dir.path().to_owned());
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 40
    })
    .await?;
    picker.results.cursor = 20;
    draw(&mut picker, &mut app);
    let selected = picker.selection().unwrap().clone();
    let old_offset = picker.results.offset;
    let mut next = paths.clone();
    next.insert(0, dir.path().join("00-new.txt"));
    picker.refresh_options(next, picker.results.editor_data.clone());
    assert_eq!(picker.selection(), Some(&selected));
    // Moving while the new matcher runs must not be undone by publication.
    picker.move_by(1, Direction::Forward);
    let selected = picker.selection().unwrap().clone();
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 41
    })
    .await?;
    assert_eq!(picker.selection(), Some(&selected));
    assert_eq!(picker.results.cursor, 22);
    assert_eq!(picker.results.offset, old_offset + 2);
    Ok(())
}

#[tokio::test]
async fn refresh_preserves_identity_and_uses_surviving_neighbours() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("name", |item: &(u32, &str), _: &()| {
            item.1.into()
        })],
        0,
        [(1, "one"), (2, "two"), (3, "three"), (4, "four")],
        (),
        |_, _, _| {},
    )
    .with_item_key(|item| item.0);
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 4
    })
    .await?;
    picker.results.cursor = 1;
    picker.refresh_options(
        vec![
            (0, "zero"),
            (1, "one"),
            (2, "changed"),
            (3, "three"),
            (4, "four"),
        ],
        picker.results.editor_data.clone(),
    );
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 5
    })
    .await?;
    assert_eq!(picker.selection(), Some(&(2, "changed")));
    // Two removed neighbours and an insertion must not select the new row or restart at one.
    picker.refresh_options(
        vec![(0, "zero"), (1, "one"), (9, "new"), (4, "four")],
        picker.results.editor_data.clone(),
    );
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 4
    })
    .await?;
    assert_eq!(picker.selection(), Some(&(4, "four")));
    picker.refresh_options(
        vec![(0, "zero"), (1, "one")],
        picker.results.editor_data.clone(),
    );
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 2
    })
    .await?;
    assert_eq!(picker.selection(), Some(&(1, "one")));
    Ok(())
}

#[tokio::test]
async fn query_edits_during_refresh_filter_the_new_rows_and_reset_selection() -> anyhow::Result<()>
{
    let mut app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("name", |item: &(u32, &str), _: &()| {
            item.1.into()
        })],
        0,
        [(1, "same"), (2, "same"), (3, "other")],
        (),
        |_, _, _| {},
    )
    .with_item_key(|item| item.1);
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 3
    })
    .await?;
    picker.results.cursor = 1;
    picker.refresh_options(
        vec![(0, "new"), (1, "same"), (2, "same"), (3, "other")],
        picker.results.editor_data.clone(),
    );
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 4
    })
    .await?;
    assert_eq!(picker.selection(), Some(&(2, "same")));
    picker.refresh_options(
        vec![(1, "same"), (2, "same"), (4, "other-new")],
        picker.results.editor_data.clone(),
    );
    picker.prompt.set_line("other".into(), &app.editor);
    picker.handle_prompt_change(false);
    until(&mut picker, &mut app, |picker| {
        picker.selection() == Some(&(4, "other-new"))
    })
    .await?;
    assert_eq!(picker.results.cursor, 0);
    assert_eq!(picker.results.matcher.snapshot().matched_item_count(), 1);
    Ok(())
}

#[tokio::test]
async fn unsaved_document_changes_refresh_without_filesystem_events_or_polling(
) -> anyhow::Result<()> {
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    let root = stdx::path::canonicalize(dir.path());
    let path = root.join("file.txt");
    std::fs::write(&path, "old")?;
    let document = app.editor.open(&path, view::editor::Action::Replace)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("length", |length: &usize, _: &PathBuf| {
            length.to_string().into()
        })],
        0,
        [3],
        root,
        |_, _, _| {},
    )
    .with_file_updates(
        &app.editor,
        |root| root,
        |_, editor, root, _| {
            let length = editor
                .document_by_path(root.join("file.txt"))
                .unwrap()
                .text()
                .len_chars();
            Box::pin(async move { Ok(vec![length]) })
        },
    )
    .refresh_on_document_changes();
    until(&mut picker, &mut app, |picker| {
        picker.selection() == Some(&3)
    })
    .await?;
    let (view, doc) = view::current!(app.editor);
    let edit = editor_core::Transaction::insert(doc.text(), doc.selection(view.id), "new".into());
    view::editing::apply(doc, &edit, view.id).unwrap();
    until(&mut picker, &mut app, |picker| {
        picker.selection() == Some(&6)
    })
    .await?;
    assert!(app.editor.document(document).unwrap().is_modified());
    assert_eq!(std::fs::read_to_string(&path)?, "old");
    Ok(())
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
    );
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 1
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
        picker.results.matcher.snapshot().item_count() == 2
    })
    .await?;
    picker.prompt.set_line("keep".into(), &app.editor);
    picker.handle_prompt_change(false);
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().matched_item_count() == 2
    })
    .await?;
    picker.results.cursor = 1;
    let selected = picker.selection().unwrap().clone();
    std::fs::write(root.join("a-keep.txt"), "a")?;
    changed(&mut app.editor, &root.join("a-keep.txt"), EventType::Create);
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 3
    })
    .await?;
    let version = picker.results.version.load(atomic::Ordering::Relaxed);
    changed(&mut app.editor, &root.join("a-keep.txt"), EventType::Create);
    until(&mut picker, &mut app, |picker| {
        picker.file_updates.as_ref().unwrap().pending.is_none()
    })
    .await?;
    assert_eq!(
        picker.results.version.load(atomic::Ordering::Relaxed),
        version
    );
    assert_eq!(picker.primary_query().as_ref(), "keep");
    assert_eq!(picker.selection(), Some(&selected));
    assert_eq!(picker.results.cursor, 2);
    std::fs::remove_file(&selected)?;
    changed(&mut app.editor, &selected, EventType::Delete);
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 2
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
        picker.results.matcher.snapshot().item_count() == 2
    })
    .await?;
    let hidden = dir.path().join(".hidden.txt");
    let ignored = dir.path().join("ignored.txt");
    std::fs::write(&hidden, "hidden")?;
    std::fs::write(&ignored, "ignored")?;
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 4
    })
    .await?;
    let renamed = dir.path().join("renamed.txt");
    std::fs::rename(&hidden, &renamed)?;
    until(&mut picker, &mut app, |picker| {
        picker
            .results
            .matcher
            .snapshot()
            .matched_items(0..picker.results.matcher.snapshot().matched_item_count())
            .any(|item| item.data.0 == renamed)
    })
    .await?;
    std::fs::remove_file(&ignored)?;
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 3
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
    );
    changed(&mut app.editor, &dir.path().join("new"), EventType::Create);
    until(&mut picker, &mut app, |picker| {
        picker.file_updates.as_ref().unwrap().pending.is_some()
    })
    .await?;
    let worker = tokio::time::timeout(Duration::from_secs(5), workers.recv())
        .await?
        .unwrap();
    let started = picker.results.editor_data.started.clone();
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
    picker
        .results
        .version
        .fetch_add(1, atomic::Ordering::Relaxed);
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
    )
    .refresh_on_content_changes();
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 2
    })
    .await?;
    picker.results.cursor = 1;
    changed(&mut app.editor, &path, EventType::Modified);
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 3
    })
    .await?;
    assert_eq!(picker.selection(), Some(&(path, 1)));
    assert_eq!(picker.results.cursor, 2);
    Ok(())
}

#[tokio::test]
async fn vcs_metadata_outside_the_worktree_refreshes_without_polling() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let worktree = tempfile::tempdir()?;
    let metadata = tempfile::tempdir()?;
    let index = metadata.path().join("index");
    std::fs::write(&index, "index")?;
    app.editor
        .file_watcher
        .set_extra_watched_paths(vec![index.clone()]);
    let latest = worktree.path().join("latest.txt");
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("path", |path: &PathBuf, _: &PathBuf| {
            path.to_string_lossy().into_owned().into()
        })],
        0,
        [worktree.path().join("old.txt")],
        worktree.path().to_owned(),
        |_, _, _| {},
    )
    .with_file_updates(
        &app.editor,
        |root| root,
        |_, _, root, _| Box::pin(async move { Ok(vec![root.join("latest.txt")]) }),
    )
    .refresh_on_vcs_changes();
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().item_count() == 1
    })
    .await?;
    changed(&mut app.editor, &index, EventType::Modified);
    until(&mut picker, &mut app, |picker| {
        picker.selection() == Some(&latest)
    })
    .await?;
    Ok(())
}
