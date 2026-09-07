use std::time::Duration;

use editor_core::Transaction;
use view::{current, doc};

use super::helpers::{run_event_loop_until_idle, AppBuilder};

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
            0..doc.text().len_bytes() as u32
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
    assert!(doc.apply(&change, view.id));

    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;

    let doc = doc!(app.editor);
    assert!(!doc.is_syntax_pending());
    let syntax = doc.syntax().expect("event loop should publish syntax");
    assert_eq!(
        syntax.tree().root_node().byte_range(),
        0..doc.text().len_bytes() as u32
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
