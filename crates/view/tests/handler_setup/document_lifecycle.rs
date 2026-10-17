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

#[tokio::test(flavor = "multi_thread")]
async fn scratch_copies_keep_source_buffers_and_require_a_save_destination() -> anyhow::Result<()> {
    let mut fixture = Fixture::with_languages(
        "source\n",
        r#"[[language]]
        name = "words"
        scope = "text.words"
        roots = []
        file-types = ["words"]"#,
    )?;
    let source = current_ref!(fixture.editor).1;
    let source_id = source.id();
    let path = source.path().unwrap().to_owned();
    fixture.replace("unsaved source\n");
    let copy = fixture.editor.open_scratch(&path, Action::Replace)?;
    assert_ne!(copy, source_id);
    let doc = fixture.editor.document(copy).unwrap();
    assert!(doc.path().is_none());
    assert!(doc.language_servers().next().is_none());
    assert_eq!(doc.language_name(), Some("words"));
    assert_eq!(doc.text(), "source\n");
    assert!(!doc.is_modified());
    assert!(fixture
        .editor
        .save(copy, None::<&std::path::Path>, false)
        .is_err());
    let source = fixture.editor.document(source_id).unwrap();
    assert_eq!(source.path(), Some(path.as_path()));
    assert_eq!(source.text(), "unsaved source\n");
    assert!(source.is_modified());
    fixture.replace_and_commit("edited copy\n");
    let destination = fixture.dir.path().join("copy.words");
    fixture.editor.save(copy, Some(&destination), false)?;
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(destination)?, "edited copy\n");
    assert_eq!(std::fs::read_to_string(path)?, "source\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scratch_copies_reject_missing_and_binary_sources_before_changing_views(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let original = current_ref!(fixture.editor).1.id();
    let binary = fixture.dir.path().join("binary.png");
    std::fs::write(&binary, b"\x89PNG\r\n\x1a\n\0")?;
    for path in [fixture.dir.path().join("missing.words"), binary] {
        assert!(fixture.editor.open_scratch(&path, Action::Replace).is_err());
        assert_eq!(current_ref!(fixture.editor).1.id(), original);
        assert_eq!(current_ref!(fixture.editor).1.text(), "original\n");
    }
    Ok(())
}
