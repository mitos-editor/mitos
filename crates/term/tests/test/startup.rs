use std::time::Duration;

use editor_core::Transaction;
use view::{current, doc};

use super::helpers::{run_event_loop_until_idle, test_config, AppBuilder};

#[tokio::test(flavor = "multi_thread")]
async fn idle_wait_consumes_queued_input_even_when_the_timer_has_expired() -> anyhow::Result<()> {
    let mut config = test_config();
    // Make input and idle ready together, as can happen on a busy CI runner.
    config.editor.idle_timeout = Duration::ZERO;
    let mut app = AppBuilder::new().with_config(config).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut input = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    let text = "queued input ".repeat(16);
    for key in ui_core::input::parse_macro(&format!("i{text}<esc>"))? {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(key.into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(key.into());
        tx.send(Ok(event))?;
    }
    assert!(
        tokio::time::timeout(
            Duration::from_secs(10),
            app.event_loop_until_idle(&mut input)
        )
        .await?
    );
    assert!(input.as_ref().is_empty(), "idle left queued input unread");
    assert_eq!(
        doc!(app.editor).text().to_string(),
        format!("{text}{}", editor_core::NATIVE_LINE_ENDING.as_str())
    );
    assert_eq!(app.editor.mode(), view::document::Mode::Normal);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_completion_retries_and_handles_closed_documents_without_terminal_jobs(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("queued.json");
    std::fs::write(&path, "{}\n")?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let blocking_tx = tx.clone();
    let sender = view::callbacks::EditorCallbackSender::new(
        event::Redraw::default(),
        move |callback| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(callback).await;
            }
        },
        move |callback| event::send_blocking(&blocking_tx, callback),
    );
    let mut app = AppBuilder::new()
        .with_file(&path, None)
        .with_handler_setup(move |handlers, _| {
            handlers.syntax = view::features::syntax::SyntaxHandler::new(sender);
        })
        .build()?;
    let id = doc!(app.editor).id();

    let callback = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await?
        .expect("syntax completion was not delivered");
    let (view, doc) = current!(app.editor);
    assert!(doc.is_syntax_pending());
    assert!(doc.syntax().is_none());
    let change = Transaction::change(
        doc.text(),
        [(1, 1, Some("\"updated\": true".into()))].into_iter(),
    );
    view::editing::apply(doc, &change, view.id).unwrap();

    // Publishing an old snapshot must request another one through the same sender.
    callback(&mut app.editor);
    assert!(doc!(app.editor).syntax().is_none());
    let callback = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await?
        .expect("syntax retry was not delivered");
    assert!(app.editor.close_document(id, true).is_ok());
    callback(&mut app.editor);
    assert!(app.editor.document(id).is_none());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_completions_stay_with_simultaneously_open_editors() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let first_path = dir.path().join("first.json");
    let second_path = dir.path().join("second.json");
    std::fs::write(&first_path, "{}\n")?;
    std::fs::write(&second_path, "{\"second\": true}\n")?;
    let mut first = AppBuilder::new().build()?;
    let mut second = AppBuilder::new().build()?;

    // The second application has replaced the global dispatch queue. Requests
    // started by the first application must still return to its own editor.
    first
        .editor
        .open(&first_path, view::editor::Action::Replace)?;
    second
        .editor
        .open(&second_path, view::editor::Action::Replace)?;
    assert_eq!(doc!(first.editor).id(), doc!(second.editor).id());

    // Force a retry on the first editor so restarted requests also prove ownership.
    let (view, doc) = current!(first.editor);
    let change = Transaction::change(
        doc.text(),
        [(1, 1, Some("\"first\": 1".into()))].into_iter(),
    );
    view::editing::apply(doc, &change, view.id).unwrap();

    for app in [&mut first, &mut second] {
        tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(app)).await?;
        let doc = doc!(app.editor);
        assert!(!doc.is_syntax_pending());
        assert_eq!(
            doc.syntax().unwrap().tree().root_node().byte_range(),
            0..doc.text().len() as u32
        );
    }
    assert_eq!(doc!(first.editor).text().to_string(), "{\"first\": 1}\n");
    assert_eq!(
        doc!(second.editor).text().to_string(),
        "{\"second\": true}\n"
    );
    assert!(first.close().await.is_empty());
    assert!(second.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_an_editor_in_the_same_runtime_receives_syntax_callbacks() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("startup.json");
    std::fs::write(&path, "{}\n")?;
    for _ in 0..2 {
        let mut app = AppBuilder::new().with_file(&path, None).build()?;
        tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;
        assert!(doc!(app.editor).syntax().is_some());
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_callbacks_update_their_own_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let first = dir.path().join("first.json");
    let second = dir.path().join("second.json");
    std::fs::write(&first, "{}\n")?;
    std::fs::write(&second, "{\"second\": true}\n")?;
    let mut app = AppBuilder::new()
        .with_file(&first, None)
        .with_file(&second, None)
        .build()?;

    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;
    for path in [&first, &second] {
        let doc = app
            .editor
            .document_by_path(stdx::path::canonicalize(path))
            .unwrap();
        assert!(!doc.is_syntax_pending());
        assert_eq!(
            doc.syntax().unwrap().tree().root_node().byte_range(),
            0..doc.text().len() as u32
        );
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_a_file_publishes_syntax_for_edits_made_before_the_event_loop() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("startup.json");
    std::fs::write(&path, "{}\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;

    let (view, doc) = current!(app.editor);
    assert!(doc.is_syntax_pending());
    assert!(doc.syntax().is_none());
    let change = Transaction::change(
        doc.text(),
        [(1, 1, Some("\"ready\": true".into()))].into_iter(),
    );
    view::editing::apply(doc, &change, view.id).unwrap();

    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;

    let doc = doc!(app.editor);
    assert!(!doc.is_syntax_pending());
    let syntax = doc.syntax().expect("event loop should publish syntax");
    assert_eq!(
        syntax.tree().root_node().byte_range(),
        0..doc.text().len() as u32
    );
    assert_eq!(doc.text().to_string(), "{\"ready\": true}\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn symbol_picker_can_open_before_background_syntax_is_published() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("symbols.rs");
    let original = "fn alpha() {}\nfn banana() {}\n";
    std::fs::write(&path, original)?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    assert!(doc!(app.editor).is_syntax_pending());

    // Deliver input immediately, without first letting the event loop publish syntax.
    for key in ui_core::input::parse_macro("<space>sbanana<C-q><esc>]q")? {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(key.into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(key.into());
        app.handle_terminal_events(Ok(event)).await;
    }
    app.editor.reset_idle_timer();
    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;

    let (view, doc) = current!(app.editor);
    assert!(!doc.is_syntax_pending());
    assert_eq!(doc.text().to_string(), original);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .fragment(doc.text().slice(..)),
        "fn banana() {}"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn diagnostic_refresh_is_delivered_once_to_the_owning_application() -> anyhow::Result<()> {
    use view::{events::DiagnosticsDidChange, features::diagnostics::DiagnosticEvent};

    let mut first = AppBuilder::new().build()?;
    let mut second = AppBuilder::new().build()?;
    let (first_tx, mut first_rx) = tokio::sync::mpsc::channel(8);
    let (second_tx, mut second_rx) = tokio::sync::mpsc::channel(8);
    let (view, _) = current!(first.editor);
    view.diagnostics_handler.events = first_tx;
    let (view, _) = current!(second.editor);
    view.diagnostics_handler.events = second_tx;

    let doc = doc!(first.editor).id();
    event::dispatch(DiagnosticsDidChange {
        editor: &mut first.editor,
        doc,
    })
    .report_to(first.editor.status_sender());
    assert!(matches!(first_rx.try_recv(), Ok(DiagnosticEvent::Refresh)));
    assert!(first_rx.try_recv().is_err(), "duplicate diagnostic refresh");
    assert!(
        second_rx.try_recv().is_err(),
        "another application's refresh"
    );

    let doc = doc!(second.editor).id();
    event::dispatch(DiagnosticsDidChange {
        editor: &mut second.editor,
        doc,
    })
    .report_to(second.editor.status_sender());
    assert!(matches!(second_rx.try_recv(), Ok(DiagnosticEvent::Refresh)));
    assert!(
        second_rx.try_recv().is_err(),
        "duplicate diagnostic refresh"
    );
    assert!(
        first_rx.try_recv().is_err(),
        "another application's refresh"
    );

    assert!(first.close().await.is_empty());
    assert!(second.close().await.is_empty());
    Ok(())
}
