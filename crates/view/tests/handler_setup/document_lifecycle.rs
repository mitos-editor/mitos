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
        let focused = event
            .editor
            .tree
            .get(event.editor.tree.focus())
            .document_id();
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

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_split_removes_its_selections_from_hidden_documents() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("first\n")?;
    let original = current_ref!(fixture.editor).1.id();
    fixture.editor.switch(original, Action::VerticalSplit);
    let split = current_ref!(fixture.editor).0.id;
    let scratch = fixture.editor.new_file(Action::Replace);
    assert_eq!(
        fixture
            .editor
            .document(original)
            .unwrap()
            .selections()
            .len(),
        2
    );
    fixture.editor.close(split);
    assert_eq!(
        fixture
            .editor
            .document(original)
            .unwrap()
            .selections()
            .len(),
        1
    );
    assert_eq!(
        fixture.editor.document(scratch).unwrap().selections().len(),
        0
    );
    assert!(fixture.editor.close_document(scratch, true).is_ok());
    let replacement = fixture.editor.new_file(Action::Replace);
    assert_ne!(replacement, scratch);
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(view.document_id(), replacement);
    assert_eq!(doc.selections().len(), 1);
    assert_eq!(
        fixture
            .editor
            .document(original)
            .unwrap()
            .selections()
            .len(),
        1
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_between_workspaces_refreshes_all_derived_paths() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("text\n")?;
    let id = current_ref!(fixture.editor).1.id();
    for (workspace, name) in [("first", "before.words"), ("second", "after.words")] {
        let root = fixture.dir.path().join(workspace);
        std::fs::create_dir_all(root.join(".git"))?;
        let root = stdx::path::canonicalize(root);
        let path = root.join(name);
        fixture.editor.set_doc_path(id, &path);
        let doc = fixture.editor.document(id).unwrap();
        assert_eq!(doc.path(), Some(path.as_path()));
        assert_eq!(doc.workspace_root(), root);
        assert_eq!(doc.display_path(), Some(std::path::Path::new(name)));
        assert_eq!(doc.display_name(), name);
        assert_eq!(
            doc.relative_path(),
            Some(stdx::path::get_relative_path(&path).as_ref())
        );
        assert_eq!(doc.text(), "text\n");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_writes_keep_their_text_encoding_and_bom() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("unused\n")?;
    let path = fixture.dir.path().join("encoded.words");
    let encoded = [b"\xff\xfe".as_slice(), b"a\0\n\0"].concat();
    std::fs::write(&path, &encoded)?;
    let id = fixture.editor.open(&path, Action::Replace)?;
    let pending = fixture
        .editor
        .document_mut(id)
        .unwrap()
        .save(None::<&std::path::Path>, true)?;
    fixture.replace("newer\n");
    fixture
        .editor
        .document_mut(id)
        .unwrap()
        .set_encoding("utf-8")?;
    let saved = pending.await?;
    assert_eq!(std::fs::read(&path)?, encoded);
    assert_eq!(saved.text, "a\n");
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.text(), "newer\n");
    assert_eq!(doc.encoding(), editor_core::encoding::UTF_8);
    assert!(doc.is_modified());
    Ok(())
}
