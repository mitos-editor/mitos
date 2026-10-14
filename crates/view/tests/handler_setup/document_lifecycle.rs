use super::Fixture;
use event::register_hook;
use std::sync::{Arc, Mutex};
use view::{
    current_ref,
    editor::Action,
    events::{DocumentDidClose, DocumentFocusLost},
};

#[tokio::test(flavor = "multi_thread")]
async fn replacement_reports_the_previous_document_and_view() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("first\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let expected = (doc.id(), view.id);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        captured.lock().unwrap().push((event.doc, event.view));
        Ok(())
    });
    let incoming = fixture.editor.new_file(Action::Replace);
    assert_ne!(incoming, expected.0);
    assert_eq!(*events.lock().unwrap(), vec![expected]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn splits_and_focus_changes_report_the_originating_view() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("first\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let first = view.id;
    let id = doc.id();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        captured.lock().unwrap().push((event.doc, event.view));
        Ok(())
    });
    fixture.editor.switch(id, Action::VerticalSplit);
    let second = current_ref!(fixture.editor).0.id;
    fixture.editor.focus(first);
    assert_eq!(*events.lock().unwrap(), vec![(id, first), (id, second)]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_an_empty_scratch_reports_closed_document_after_rebinding() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new("first\n")?;
    let existing = current_ref!(fixture.editor).1.id();
    let scratch = fixture.editor.new_file(Action::Replace);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        let focused = event.editor.tree.get(event.editor.tree.focus).doc;
        assert!(event.editor.document(focused).is_some());
        captured.lock().unwrap().push((event.doc.id(), focused));
        Ok(())
    });
    let focus_events = Arc::new(Mutex::new(Vec::new()));
    let captured = focus_events.clone();
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        captured.lock().unwrap().push((event.doc, event.view));
        Ok(())
    });
    fixture.editor.switch(existing, Action::Replace);
    assert!(fixture.editor.document(scratch).is_none());
    assert_eq!(*events.lock().unwrap(), vec![(scratch, existing)]);
    assert!(focus_events.lock().unwrap().is_empty());
    Ok(())
}
