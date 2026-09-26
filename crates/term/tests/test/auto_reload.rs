use anyhow::Context as _;
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

use tempfile::TempDir;
use term::application::Application;
use view::file_watcher::{CanonicalPathBuf, Event, EventType, Events};
use view::{current, current_ref};

use super::helpers::*;

fn changed(path: &Path, text: &str, tick: u64) -> anyhow::Result<()> {
    fs::write(path, text)?;
    fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(tick))?;
    Ok(())
}

fn app(path: &Path) -> anyhow::Result<Application> {
    let mut config = test_config();
    config.editor.auto_reload.enable = true;
    AppBuilder::new()
        .with_config(config)
        .with_file(path, None)
        .build()
}

async fn notify(app: &mut Application, path: &Path, ty: EventType) {
    let path = stdx::path::canonicalize_existing(path);
    let events = Events::from(vec![Event {
        path: CanonicalPathBuf::assert_canonicalized(&path),
        ty,
    }]);
    app.editor.handle_file_events(&events);
    // Direct shared API calls do not reset the terminal loop's idle timer.
    app.editor.reset_idle_timer();
    run_event_loop_until_idle(app).await;
}

fn text(app: &Application) -> String {
    current_ref!(app.editor).1.text().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn clean_buffers_reload_and_deletion_preserves_the_buffer() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "before\n", 1)?;
    let mut app = app(&path)?;
    changed(&path, "after\n", 2)?;
    notify(&mut app, &path, EventType::Modified).await;
    assert_eq!(text(&app), "after\n");
    assert!(!current_ref!(app.editor).1.is_modified());
    fs::remove_file(&path)?;
    notify(&mut app, &path, EventType::Delete).await;
    assert_eq!(text(&app), "after\n");
    changed(&path, "recreated\n", 3)?;
    notify(&mut app, &path, EventType::Create).await;
    assert_eq!(text(&app), "recreated\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn modified_buffers_prompt_once_and_reload_only_when_confirmed() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "before\n", 1)?;
    let mut app = app(&path)?;
    {
        let (view, doc) = current!(app.editor);
        let transaction = editor_core::Transaction::change(
            doc.text(),
            [(0, 0, Some("local ".into()))].into_iter(),
        );
        doc.apply(&transaction, view.id);
        doc.append_changes_to_history(view);
    }
    changed(&path, "external\n", 2)?;
    notify(&mut app, &path, EventType::Modified).await;
    let seen = current_ref!(app.editor).1.auto_reload_seen_mtime;
    assert!(seen.is_some());
    assert_eq!(text(&app), "local before\n");
    notify(&mut app, &path, EventType::Modified).await;
    assert_eq!(current_ref!(app.editor).1.auto_reload_seen_mtime, seen);
    // One Escape must dismiss the only prompt. A second external edit prompts again.
    #[cfg(not(windows))]
    let escape = termina::event::Event::Key(ui_core::input::parse_macro("<esc>")?[0].into());
    #[cfg(windows)]
    let escape = crossterm::event::Event::Key(ui_core::input::parse_macro("<esc>")?[0].into());
    app.handle_terminal_events(Ok(escape)).await;
    assert_eq!(text(&app), "local before\n");
    changed(&path, "new external\n", 3)?;
    notify(&mut app, &path, EventType::Modified).await;
    test_key_sequence(
        &mut app,
        Some("<ret>"),
        Some(&|app| {
            assert_eq!(text(app), "new external\n");
            assert!(!current_ref!(app.editor).1.is_modified());
        }),
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_reloads_unwatched_files_and_own_saves_do_not_conflict() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "before\n", 1)?;
    let mut app = app(&path)?;
    changed(&path, "focus\n", 2)?;
    #[cfg(not(windows))]
    app.handle_terminal_events(Ok(termina::event::Event::FocusIn))
        .await;
    #[cfg(windows)]
    app.handle_terminal_events(Ok(crossterm::event::Event::FocusGained))
        .await;
    run_event_loop_until_idle(&mut app).await;
    assert_eq!(text(&app), "focus\n");
    let id = current_ref!(app.editor).1.id();
    app.editor.save(id, None::<&Path>, false)?;
    app.editor.flush_writes().await?;
    notify(&mut app, &path, EventType::Modified).await;
    assert!(current_ref!(app.editor).1.auto_reload_seen_mtime.is_none());
    assert!(!current_ref!(app.editor).1.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_watcher_detects_atomic_replacement() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "before\n", 1)?;
    let mut app = app(&path)?;
    app.editor
        .file_watcher
        .reload(&view::file_watcher::Config::default());
    app.editor.file_watcher.add_root(dir.path());
    tokio::time::timeout(Duration::from_secs(10), async {
        while !app.editor.file_watcher.is_watching(&path) {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await
    .context("native watch root did not become ready")?;
    let replacement = dir.path().join("replacement");
    changed(&replacement, "replacement\n", 2)?;
    fs::rename(replacement, &path)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while text(&app) != "replacement\n" {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await
    .context("atomic replacement did not reload the buffer")?;
    assert!(!current_ref!(app.editor).1.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn polling_starts_after_config_reload_and_can_be_disabled() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "before\n", 1)?;
    let mut app = app(&path)?;
    let mut config = (*app.editor.config()).clone();
    config.auto_reload.poll.enable = true;
    config.auto_reload.poll.interval = 100;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config.clone())));
    changed(&path, "polled\n", 2)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while text(&app) != "polled\n" {
            // The periodic poll itself keeps resetting the application's idle timer.
            let _ = tokio::time::timeout(
                Duration::from_millis(150),
                run_event_loop_until_idle(&mut app),
            )
            .await;
        }
    })
    .await?;
    config.auto_reload.enable = false;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    changed(&path, "disabled\n", 3)?;
    notify(&mut app, &path, EventType::Modified).await;
    assert_eq!(text(&app), "polled\n");
    Ok(())
}

struct SharedReload {
    app: Application,
    callbacks: tokio::sync::mpsc::UnboundedReceiver<view::callbacks::EditorCallback>,
}

impl SharedReload {
    fn new(path: &Path) -> anyhow::Result<Self> {
        let app = app(path)?;
        let (_, callbacks) = tokio::sync::mpsc::unbounded_channel();
        let mut fixture = Self { app, callbacks };
        fixture.replace_handler();
        Ok(fixture)
    }

    fn replace_handler(&mut self) {
        use view::{callbacks::EditorCallbackSender, handlers::auto_reload::AutoReloadHandler};
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let blocking = tx.clone();
        let callbacks = EditorCallbackSender::new(
            move |callback| {
                let _ = tx.send(callback);
                async {}
            },
            move |callback| {
                let _ = blocking.send(callback);
            },
        );
        self.app.editor.handlers.auto_reload =
            AutoReloadHandler::new(callbacks.clone(), &self.app.editor.config());
        self.app.editor.file_watcher =
            view::file_watcher::Watcher::new(&self.app.editor.config().file_watcher, callbacks);
        self.callbacks = rx;
    }

    async fn callback(&mut self) -> anyhow::Result<view::callbacks::EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("reload callback queue closed")
    }
}

fn fs_events(paths: &[&Path]) -> Events {
    Events::from(
        paths
            .iter()
            .map(|path| {
                let path = stdx::path::canonicalize_existing(path);
                Event {
                    path: CanonicalPathBuf::assert_canonicalized(&path),
                    ty: EventType::Modified,
                }
            })
            .collect::<Vec<_>>(),
    )
}

fn edit_buffer(app: &mut Application, prefix: &str) {
    let (view, doc) = current!(app.editor);
    let transaction =
        editor_core::Transaction::change(doc.text(), [(0, 0, Some(prefix.into()))].into_iter());
    assert!(doc.apply(&transaction, view.id));
    doc.append_changes_to_history(view);
}

#[tokio::test(flavor = "multi_thread")]
async fn native_events_stay_with_their_editor_and_old_watchers_cannot_apply_callbacks(
) -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("shared.txt");
    changed(&path, "original\n", 1)?;
    let mut first = SharedReload::new(&path)?;
    let mut second = SharedReload::new(&path)?;
    // Both editors have the same file open. Only the first owns a native watch.
    first
        .app
        .editor
        .file_watcher
        .reload(&view::file_watcher::Config::default());
    first.app.editor.file_watcher.add_root(dir.path());
    tokio::time::timeout(Duration::from_secs(10), async {
        while !first.app.editor.file_watcher.is_watching(&path) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    changed(&path, "first update\n", 2)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while text(&first.app) != "first update\n" {
            let callback = first.callback().await?;
            callback(&mut first.app.editor);
        }
        anyhow::Ok(())
    })
    .await??;
    assert_eq!(text(&second.app), "original\n");
    assert!(second.callbacks.try_recv().is_err());
    // Clear callbacks from the initial crawl before collecting a new change.
    while let Ok(callback) = first.callbacks.try_recv() {
        callback(&mut first.app.editor);
    }
    changed(&path, "queued update\n", 3)?;
    let stale = first.callback().await?;
    // Even a callback delivered to the wrong editor must be harmless.
    stale(&mut second.app.editor);
    assert_eq!(text(&second.app), "original\n");
    changed(&path, "disabled watcher\n", 4)?;
    let stale = first.callback().await?;
    first
        .app
        .editor
        .file_watcher
        .reload(&view::file_watcher::Config {
            enable: false,
            ..Default::default()
        });
    stale(&mut first.app.editor);
    assert_eq!(text(&first.app), "first update\n");
    while let Ok(callback) = first.callbacks.try_recv() {
        callback(&mut first.app.editor);
    }
    assert_eq!(text(&first.app), "first update\n");
    // Replacing an active watcher must also reject callbacks already queued.
    first
        .app
        .editor
        .file_watcher
        .reload(&view::file_watcher::Config::default());
    changed(&path, "replacement watcher\n", 5)?;
    let stale = first.callback().await?;
    first.replace_handler();
    stale(&mut first.app.editor);
    assert_eq!(text(&first.app), "first update\n");
    first.app.editor.handle_file_events(&fs_events(&[&path]));
    assert_eq!(text(&first.app), "replacement watcher\n");
    assert!(first.app.close().await.is_empty());
    drop(first.app);
    assert!(first.callbacks.recv().await.is_none());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn polling_uses_its_owner_and_queued_polls_respect_disabled_settings() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let first_path = dir.path().join("first.txt");
    let second_path = dir.path().join("second.txt");
    changed(&first_path, "first\n", 1)?;
    changed(&second_path, "second\n", 1)?;
    let mut first = SharedReload::new(&first_path)?;
    let mut second = SharedReload::new(&second_path)?;
    for f in [&mut first, &mut second] {
        let mut config = (*f.app.editor.config()).clone();
        config.auto_reload.poll.enable = true;
        config.auto_reload.poll.interval = 100;
        f.app
            .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    }
    changed(&first_path, "polled first\n", 2)?;
    changed(&second_path, "polled second\n", 2)?;
    let callback = first.callback().await?;
    callback(&mut first.app.editor);
    assert_eq!(text(&first.app), "polled first\n");
    assert_eq!(text(&second.app), "second\n");
    let callback = second.callback().await?;
    let mut config = (*second.app.editor.config()).clone();
    config.auto_reload.poll.enable = false;
    second
        .app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    callback(&mut second.app.editor);
    assert_eq!(text(&second.app), "second\n");
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_confirmations_cannot_overwrite_edited_renamed_closed_saved_or_changed_files(
) -> anyhow::Result<()> {
    use view::handlers::auto_reload::{
        check_unwatched, next_reload_request, resolve_reload, ReloadDecision,
    };
    for mutation in ["edit", "rename", "close", "save", "disk", "disable"] {
        let dir = TempDir::new()?;
        let path = dir.path().join("file.txt");
        changed(&path, "original\n", 1)?;
        let mut app = app(&path)?;
        edit_buffer(&mut app, "local ");
        changed(&path, "external\n", 2)?;
        check_unwatched(&mut app.editor);
        let request =
            next_reload_request(&mut app.editor).expect("modified buffer needs confirmation");
        let id = current_ref!(app.editor).1.id();
        match mutation {
            "edit" => edit_buffer(&mut app, "newer "),
            "rename" => {
                current!(app.editor)
                    .1
                    .set_path(Some(&dir.path().join("renamed.txt")));
            }
            "close" => assert!(app.editor.close_document(id, true).is_ok()),
            "save" => {
                app.editor.save(id, None::<&Path>, true)?;
                app.editor.flush_writes().await?;
            }
            "disk" => changed(&path, "newer external\n", 3)?,
            "disable" => {
                let mut config = (*app.editor.config()).clone();
                config.auto_reload.enable = false;
                app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
            }
            _ => unreachable!(),
        }
        let before = app.editor.document(id).map(|doc| doc.text().to_string());
        resolve_reload(&mut app.editor, request, ReloadDecision::Reload);
        assert_eq!(
            app.editor.document(id).map(|doc| doc.text().to_string()),
            before,
            "{mutation}"
        );
        if matches!(mutation, "edit" | "disk") {
            check_unwatched(&mut app.editor);
            assert!(
                next_reload_request(&mut app.editor).is_some(),
                "stale {mutation} must permit a fresh request"
            );
        }
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmations_belong_to_their_editor_and_newer_requests_survive_old_answers(
) -> anyhow::Result<()> {
    use view::handlers::auto_reload::{
        check_unwatched, next_reload_request, resolve_reload, ReloadDecision,
    };
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "original\n", 1)?;
    let mut first = app(&path)?;
    let mut second = app(&path)?;
    edit_buffer(&mut first, "local ");
    edit_buffer(&mut second, "local ");
    changed(&path, "external\n", 2)?;
    for app in [&mut first, &mut second] {
        check_unwatched(&mut app.editor);
    }
    let wrong_owner = next_reload_request(&mut first.editor).unwrap();
    resolve_reload(&mut second.editor, wrong_owner, ReloadDecision::Reload);
    assert_eq!(text(&second), "local original\n");
    assert!(next_reload_request(&mut second.editor).is_some());

    changed(&path, "new\n", 3)?;
    check_unwatched(&mut first.editor);
    let old = next_reload_request(&mut first.editor).unwrap();
    changed(&path, "newest\n", 4)?;
    check_unwatched(&mut first.editor);
    let newest = next_reload_request(&mut first.editor).unwrap();
    resolve_reload(&mut first.editor, old, ReloadDecision::Ignore);
    assert_eq!(text(&first), "local original\n");
    resolve_reload(&mut first.editor, newest, ReloadDecision::Reload);
    assert_eq!(text(&first), "newest\n");
    // Requests can also become stale before a frontend has displayed them.
    edit_buffer(&mut first, "local ");
    changed(&path, "queued old\n", 5)?;
    check_unwatched(&mut first.editor);
    changed(&path, "queued new\n", 6)?;
    check_unwatched(&mut first.editor);
    let current = next_reload_request(&mut first.editor).unwrap();
    resolve_reload(&mut first.editor, current, ReloadDecision::Reload);
    assert_eq!(text(&first), "queued new\n");
    assert!(next_reload_request(&mut first.editor).is_none());
    assert!(first.close().await.is_empty());
    assert!(second.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_reload_preserves_undo_and_synchronizes_all_splits() -> anyhow::Result<()> {
    use view::handlers::auto_reload::{
        check_unwatched, next_reload_request, resolve_reload, ReloadDecision,
    };
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "original text\n", 1)?;
    let mut app = app(&path)?;
    app.editor
        .open(&path, view::editor::Action::VerticalSplit)?;
    edit_buffer(&mut app, "local ");
    for (view, _) in app.editor.tree.views() {
        let doc = app.editor.documents.get_mut(&view.doc).unwrap();
        doc.set_selection(
            view.id,
            editor_core::Selection::point(doc.text().len_chars() - 2),
        );
    }
    changed(&path, "new\n", 2)?;
    check_unwatched(&mut app.editor);
    let request = next_reload_request(&mut app.editor).unwrap();
    resolve_reload(&mut app.editor, request, ReloadDecision::Reload);
    assert_eq!(text(&app), "new\n");
    assert!(!current_ref!(app.editor).1.is_modified());
    let scrolloff = app.editor.config().scrolloff;
    for (view, _) in app.editor.tree.views_mut() {
        let doc = &app.editor.documents[&view.doc];
        assert!(doc.selection(view.id).primary().head <= doc.text().len_chars());
        assert!(view.is_cursor_in_view(doc, scrolloff));
    }
    let (view, doc) = current!(app.editor);
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "local original text\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_filesystem_and_polling_paths_refresh_vcs_with_auto_reload_disabled(
) -> anyhow::Result<()> {
    use view::handlers::auto_reload::check_unwatched;
    let dir = TempDir::new()?;
    let path = dir.path().join("file.txt");
    changed(&path, "original\n", 1)?;
    let git = |args: &[&str]| -> anyhow::Result<()> {
        let output = std::process::Command::new("git")
            .current_dir(dir.path())
            .args([
                "-c",
                "user.name=Reload Test",
                "-c",
                "user.email=reload@example.test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.fsmonitor=false",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                dir.path().join("hooks").display()
            ))
            .args(args)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    git(&["init", "--quiet", "--initial-branch=first"])?;
    git(&["add", "file.txt"])?;
    git(&["commit", "--quiet", "-m", "fixture"])?;
    git(&["branch", "second"])?;
    let mut config = test_config();
    config.editor.file_watcher.watch_vcs = true;
    config.editor.auto_reload.enable = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    // Restricted workspaces still support repository reads through reduced Git trust.
    app.editor
        .workspace_trust
        .set_config(loader::workspace_trust::Config::default());
    let head = dir.path().join(".git/HEAD");
    changed(&head, "ref: refs/heads/second\n", 2)?;
    app.editor.handle_file_events(&fs_events(&[&head]));
    assert_eq!(
        current_ref!(app.editor)
            .1
            .version_control_head()
            .unwrap()
            .as_ref()
            .as_ref(),
        "second"
    );
    assert!(app
        .editor
        .file_watcher
        .is_vcs_path(&dir.path().join(".git/refs/heads/second").canonicalize()?));
    changed(&head, "ref: refs/heads/first\n", 3)?;
    check_unwatched(&mut app.editor);
    assert_eq!(
        current_ref!(app.editor)
            .1
            .version_control_head()
            .unwrap()
            .as_ref()
            .as_ref(),
        "first"
    );
    assert!(app
        .editor
        .file_watcher
        .is_vcs_path(&dir.path().join(".git/refs/heads/first").canonicalize()?));
    assert_eq!(text(&app), "original\n");
    assert!(app.close().await.is_empty());
    Ok(())
}
