use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
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

struct FirstFrame(Arc<AtomicBool>);

impl Component for FirstFrame {
    fn render(&mut self, _area: Rect, _frame: &mut tui::buffer::Buffer, cx: &mut Context) {
        assert!(doc!(cx.editor).syntax().is_some());
        self.0.store(true, Ordering::Relaxed);
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

    let rendered = Arc::new(AtomicBool::new(false));
    app.compositor.push(Box::new(FirstFrame(rendered.clone())));
    let mut input = app.event_stream();
    // Poll through the first render and stop when the event loop waits for work.
    assert!(app.event_loop(&mut input).now_or_never().is_none());
    assert!(rendered.load(Ordering::Relaxed));
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

    let rendered = Arc::new(AtomicBool::new(false));
    app.compositor.push(Box::new(FirstFrame(rendered.clone())));
    let key = ui_core::input::parse_macro("l")?
        .into_iter()
        .next()
        .unwrap();
    #[cfg(not(windows))]
    let event = termina::event::Event::Key(key.into());
    #[cfg(windows)]
    let event = crossterm::event::Event::Key(key.into());
    app.handle_terminal_events(Ok(event)).await;
    assert!(rendered.load(Ordering::Relaxed));
    assert!(app.close().await.is_empty());
    Ok(())
}
