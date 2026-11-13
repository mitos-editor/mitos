use super::*;
use editor_core::{
    movement::{Direction, Movement},
    Transaction,
};
use view::{
    editor::{Action, SearchOutcome},
    features::buffer_search::Query,
};

async fn output(f: &mut Fixture, id: DebugAdapterId, category: &str, text: &str) -> bool {
    f.editor
        .handle_debugger_message(
            id,
            debugger_event("output", json!({"category":category,"output":text})),
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn transcripts_stream_without_focus_and_remain_searchable_after_closing_their_buffer(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("source\n")?;
    let source = view::current_ref!(f.editor).1.id();
    let (id, _wire) = adapter(&mut f).await?;
    f.editor.set_status("debugging");
    output(&mut f, id, "stdout", "hel").await;
    output(&mut f, id, "stdout", "lo\n").await;
    output(&mut f, id, "stderr", "failure\n").await;
    assert!(!output(&mut f, id, "telemetry", "hidden\n").await);
    assert_eq!(view::current_ref!(f.editor).1.id(), source);
    assert_eq!(f.editor.get_status().unwrap().0.as_ref(), "debugging");

    let document = f.editor.open_debugger_output(Action::Replace)?;
    let (current, doc) = view::current_ref!(f.editor);
    assert_eq!(doc.text(), "[stdout] hello\n[stderr] failure\n");
    assert!(doc.is_content_readonly());
    assert!(doc.path().is_none());
    assert!(!doc.is_modified());
    let edit = Transaction::insert(doc.text(), doc.selection(current.id), "changed".into());
    assert_eq!(
        f.editor
            .document_mut(document)
            .unwrap()
            .apply(edit.changes()),
        Err(view::editing::EditError::ReadOnlyDocument)
    );
    assert!(matches!(
        f.editor.search_buffer(
            &Query::new("failure", true, false)?,
            Movement::Move,
            Direction::Forward,
            1,
            true,
        ),
        SearchOutcome::Found { .. }
    ));

    assert!(f.editor.close_document(document, false).is_ok());
    output(&mut f, id, "console", "more\n").await;
    assert_eq!(view::current_ref!(f.editor).1.id(), source);
    let reopened = f.editor.open_debugger_output(Action::Replace)?;
    assert_ne!(reopened, document);
    assert_eq!(
        f.editor.document(reopened).unwrap().text(),
        "[stdout] hello\n[stderr] failure\n[console] more\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn independent_transcripts_survive_transport_close_and_select_the_most_recently_ended_session(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let source = view::current_ref!(f.editor).1.id();
    let (first, _first_wire) = adapter(&mut f).await?;
    let first_number = f.editor.debugger().get_session(first).unwrap().number();
    output(&mut f, first, "stdout", "first\n").await;
    let (second, _second_wire) = adapter(&mut f).await?;
    output(&mut f, second, "stdout", "second\n").await;
    output(&mut f, first, "stderr", "first error\n").await;
    assert_eq!(view::current_ref!(f.editor).1.id(), source);
    assert!(f
        .editor
        .debugger()
        .output_sessions()
        .iter()
        .all(|session| session.is_live));
    let second_document = f.editor.open_debugger_output(Action::Replace)?;
    assert_eq!(
        f.editor.document(second_document).unwrap().text(),
        "[stdout] second\n"
    );
    f.editor
        .handle_debugger_event(first, dap::ClientEvent::Closed { error: None })
        .await;
    f.editor
        .handle_debugger_event(second, dap::ClientEvent::Closed { error: None })
        .await;
    assert_eq!(
        f.editor.open_debugger_output(Action::Replace)?,
        second_document
    );
    assert!(f
        .editor
        .debugger()
        .output_sessions()
        .iter()
        .all(|session| !session.is_live));
    let first_document = f
        .editor
        .open_debugger_output_for_session(first_number, Action::Replace)?;
    assert_ne!(first_document, second_document);
    assert_eq!(
        f.editor.document(first_document).unwrap().text(),
        "[stdout] first\n[stderr] first error\n"
    );
    assert!(f.editor.close_document(second_document, false).is_ok());
    let reopened = f.editor.open_debugger_output(Action::Replace)?;
    assert_eq!(
        f.editor.document(reopened).unwrap().text(),
        "[stdout] second\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn retained_transcripts_evict_old_completed_sessions_without_discarding_live_output(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (live, _wire) = adapter(&mut f).await?;
    output(&mut f, live, "stdout", "live\n").await;
    let live_document = f.editor.open_debugger_output(Action::Replace)?;
    let mut ended_documents = Vec::new();
    for _ in 0..10 {
        let (id, _wire) = adapter(&mut f).await?;
        output(&mut f, id, "stdout", "completed\n").await;
        ended_documents.push(f.editor.open_debugger_output(Action::Replace)?);
        f.editor
            .handle_debugger_event(id, dap::ClientEvent::Closed { error: None })
            .await;
    }
    for document in &ended_documents[..2] {
        assert!(f.editor.document(*document).is_none());
    }
    for document in &ended_documents[2..] {
        assert!(f.editor.document(*document).is_some());
    }
    assert!(f.editor.document(live_document).is_some());
    assert_eq!(
        f.editor.open_debugger_output(Action::Replace)?,
        live_document
    );
    Ok(())
}
