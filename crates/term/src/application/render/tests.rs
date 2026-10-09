use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use editor_core::syntax;
use futures_util::FutureExt;
use view::{callbacks::EditorCallback, graphics::Rect};

use super::Application;
use crate::{
    args::Args,
    compositor::{Component, Context},
    config::Config,
};

#[derive(Clone, Default)]
struct FrameProbe(Arc<Mutex<Vec<bool>>>);

impl FrameProbe {
    fn frames(&self) -> Vec<bool> {
        self.0.lock().unwrap().clone()
    }
}

impl Component for FrameProbe {
    fn render(&mut self, _area: Rect, _frame: &mut tui::buffer::Buffer, cx: &mut Context) {
        self.0
            .lock()
            .unwrap()
            .push(doc!(cx.editor).syntax().is_some());
    }
}

fn application(
    args: Args,
) -> anyhow::Result<(Application, tokio::sync::mpsc::Receiver<EditorCallback>)> {
    let mut config = Config::default();
    config.editor.lsp.enable = false;
    config.editor.file_watcher.enable = false;
    config.editor.auto_reload.enable = false;
    config.editor.word_completion.enable = false;
    let lang_loader = syntax::Loader::new(
        toml::from_str(
            r#"
            [[language]]
            name = "json"
            scope = "source.json"
            file-types = ["json"]
            "#,
        )?,
        loader::syntax::Resources::default(),
    )?;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let blocking_tx = tx.clone();
    let sender = view::callbacks::EditorCallbackSender::new(
        move |callback| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(callback).await;
            }
        },
        move |callback| event::send_blocking(&blocking_tx, callback),
    );
    let app = Application::new_with_handler_setup(
        args,
        config,
        lang_loader,
        loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        move |handlers, _| {
            handlers.syntax = view::handlers::syntax::SyntaxHandler::new(sender);
        },
    )?;

    Ok((app, rx))
}

async fn syntax_completion(
    rx: &mut tokio::sync::mpsc::Receiver<EditorCallback>,
) -> anyhow::Result<EditorCallback> {
    Ok(tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await?
        .expect("syntax completion was not delivered"))
}

async fn pending_application(path: &Path) -> anyhow::Result<(Application, EditorCallback)> {
    std::fs::write(path, "{}\n")?;
    let mut args = Args::default();
    args.files.insert(path.to_owned(), vec![Default::default()]);
    let (app, mut rx) = application(args)?;
    let callback = syntax_completion(&mut rx).await?;
    Ok((app, callback))
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_syntax_is_published_before_first_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, callback) = pending_application(&dir.path().join("startup.json")).await?;
    assert!(doc!(app.editor).syntax().is_none());
    app.jobs.editor_callback_sender().send(callback).await;

    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let mut input = app.event_stream();
    // Poll through the first render and stop when the event loop waits for work.
    assert!(app.event_loop(&mut input).now_or_never().is_none());
    assert_eq!(frames.frames().first(), Some(&true));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_a_file_publishes_ready_syntax_before_input_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("opened.json");
    std::fs::write(&path, "{}\n")?;
    let (mut app, mut rx) = application(Args::default())?;
    app.render().await;
    app.editor.open(&path, view::editor::Action::Replace)?;
    let callback = syntax_completion(&mut rx).await?;
    assert!(doc!(app.editor).syntax().is_none());
    app.jobs.editor_callback_sender().send(callback).await;

    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let key = ui_core::input::parse_macro("l")?
        .into_iter()
        .next()
        .unwrap();
    #[cfg(not(windows))]
    let event = termina::event::Event::Key(key.into());
    #[cfg(windows)]
    let event = crossterm::event::Event::Key(key.into());
    app.handle_terminal_events(Ok(event)).await;
    assert_eq!(frames.frames().first(), Some(&true));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn syntax_finishing_during_grace_is_in_the_first_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, callback) = pending_application(&dir.path().join("startup.json")).await?;
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let sender = app.jobs.editor_callback_sender();
    tokio::time::pause();
    let start = tokio::time::Instant::now();

    {
        let render = app.render();
        tokio::pin!(render);
        assert!(render.as_mut().now_or_never().is_none());
        assert!(frames.frames().is_empty());
        tokio::time::advance(Duration::from_millis(8)).await;
        sender.send(callback).await;
        render.await;
    }

    assert_eq!(start.elapsed(), Duration::from_millis(8));
    assert_eq!(frames.frames(), [true]);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn grace_expires_after_16_ms_and_is_not_repeated() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, callback) = pending_application(&dir.path().join("startup.json")).await?;
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let sender = app.jobs.editor_callback_sender();
    tokio::time::pause();
    let start = tokio::time::Instant::now();

    {
        let render = app.render();
        tokio::pin!(render);
        assert!(render.as_mut().now_or_never().is_none());
        tokio::time::advance(Duration::from_millis(15)).await;
        // Unrelated results may be processed, but must not reset the deadline.
        sender
            .send(|editor| editor.set_status("Still loading syntax"))
            .await;
        assert!(render.as_mut().now_or_never().is_none());
        assert!(frames.frames().is_empty());
        tokio::time::advance(Duration::from_millis(1)).await;
        render.await;
    }

    // Tokio rounds timer deadlines up to its millisecond tick.
    let elapsed = start.elapsed();
    assert!((Duration::from_millis(16)..=Duration::from_millis(17)).contains(&elapsed));
    assert_eq!(frames.frames(), [false]);
    assert!(app.render().now_or_never().is_some());
    assert_eq!(frames.frames(), [false, false]);
    assert_eq!(start.elapsed(), elapsed);
    sender.send(callback).await;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn background_syntax_does_not_delay_a_plain_text_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, _callback) = pending_application(&dir.path().join("hidden.json")).await?;
    app.editor.new_file(view::editor::Action::Replace);
    tokio::time::pause();
    let start = tokio::time::Instant::now();

    assert!(app.render().now_or_never().is_some());
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert!(app.close().await.is_empty());
    Ok(())
}
