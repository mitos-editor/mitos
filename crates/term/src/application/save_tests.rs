//! Deferred lifecycle operations must preserve accepted saves and the user's current view.
use super::*;
use crate::{
    args::Args,
    config::Config,
    job::{Callback, Job},
};
use std::sync::Arc;
use view::editor::Action;

fn application() -> anyhow::Result<Application> {
    let mut config = Config::default();
    config.editor.lsp.enable = false;
    config.editor.format.on_save = false;
    config.editor.files.watch.enable = false;
    config.editor.files.reload.poll.enable = false;
    Application::new(
        Args::default(),
        config,
        language::syntax::Loader::new(
            toml::from_str("language = []")?,
            loader::syntax::Resources::default(),
        )?,
        workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
    )
}

async fn keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{Event, KeyEvent};
    #[cfg(not(windows))]
    use termina::event::{Event, KeyEvent};
    for key in ui_core::input::parse_macro(keys)? {
        app.handle_terminal_events(Ok(Event::Key(KeyEvent::from(key))))
            .await;
    }
    Ok(())
}

async fn idle(app: &mut Application) -> anyhow::Result<()> {
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            app.event_loop_until_idle(&mut app.event_stream())
        )
        .await?
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_reports_followup_failures_and_drains_other_writes() -> anyhow::Result<()> {
    let mut app = application()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("accepted.txt");
    keys(&mut app, "iaccepted<esc>").await?;
    app.editor
        .save(doc!(app.editor).id(), Some(path.clone()), false)?;
    app.jobs.add(
        Job::with_callback(async {
            Ok(Callback::Followup(Box::new(|_| {
                anyhow::bail!("followup failure")
            })))
        })
        .wait_before_exiting(),
    );
    let errors = app.close().await;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].to_string(), "followup failure");
    assert_eq!(
        std::fs::read_to_string(path)?,
        format!("accepted{}", editor_core::NATIVE_LINE_ENDING.as_str())
    );
    assert!(!app.editor.has_pending_writes());
    assert!(app.jobs.wait_futures.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_required_jobs_cancel_deferred_closes() -> anyhow::Result<()> {
    let mut app = application()?;
    let view = app.editor.tree.focus();
    app.jobs
        .add(Job::new(async { anyhow::bail!("failed save stage") }).wait_before_exiting());
    app.jobs.after_writes(
        &mut app.editor,
        Box::new(move |editor| {
            editor.close(view);
            Ok(())
        }),
    )?;
    idle(&mut app).await?;
    assert!(app.editor.tree.contains(view));
    assert!(app.editor.is_err());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_closes_do_not_follow_rebound_views() -> anyhow::Result<()> {
    for command in [":q", ":q!", ":wq", ":wq!"] {
        let mut app = application()?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("saved.txt");
        std::fs::write(&path, "original")?;
        app.editor.open(&path, Action::Replace)?;
        let (release, ready) = tokio::sync::oneshot::channel();
        app.jobs.add(
            Job::new(async {
                ready.await?;
                Ok(())
            })
            .wait_before_exiting(),
        );
        keys(&mut app, &format!("{command}<ret>")).await?;
        let replacement = app.editor.new_file(Action::Replace);
        release.send(()).unwrap();
        idle(&mut app).await?;
        assert_eq!(doc!(app.editor).id(), replacement, "{command}");
        assert!(!app.editor.should_close(), "{command}");
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forced_save_quit_preserves_edits_made_after_write_submission() -> anyhow::Result<()> {
    for command in [":wq!", ":wqa!", ":wbc!", ":x!"] {
        let mut app = application()?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("saved.txt");
        std::fs::write(&path, "original\n")?;
        app.editor.open(&path, Action::Replace)?;
        let (release, ready) = tokio::sync::oneshot::channel();
        app.jobs.add(
            Job::new(async {
                ready.await?;
                Ok(())
            })
            .wait_before_exiting(),
        );
        keys(
            &mut app,
            &format!("iaccepted<esc>{command}<ret>inewer<esc>"),
        )
        .await?;
        release.send(()).unwrap();
        idle(&mut app).await?;
        assert!(doc!(app.editor).is_modified(), "{command}");
        assert!(app.editor.is_err(), "{command}");
        assert!(!app.editor.should_close(), "{command}");
        assert!(!std::fs::read_to_string(&path)?.contains("newer"));
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_required_jobs_do_not_skip_callbacks_or_their_followups() -> anyhow::Result<()> {
    let mut app = application()?;
    let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = applied.clone();
    app.jobs
        .add(Job::new(async { anyhow::bail!("first failure") }).wait_before_exiting());
    let continuation = Job::with_callback(async move {
        Ok(Callback::Editor(Box::new(move |_| {
            captured.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        })))
    })
    .wait_before_exiting();
    let followup = Callback::Followup(Box::new(move |_| Ok(Some(continuation))));
    app.jobs
        .add(Job::with_callback(async { Ok(followup) }).wait_before_exiting());
    let errors = app.close().await;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].to_string(), "first failure");
    assert_eq!(applied.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(app.jobs.wait_futures.is_empty());
    assert!(app.close().await.is_empty());
    Ok(())
}
