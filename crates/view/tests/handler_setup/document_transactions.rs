use super::Fixture;
use editor_core::{Selection, Transaction};
use view::{current, current_ref, editor::Action};

#[tokio::test(flavor = "multi_thread")]
async fn hidden_document_edits_undo_without_changing_the_focused_document() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("ßéx\n")?;
    let id = current_ref!(fixture.editor).1.id();
    fixture.editor.new_file(Action::Replace);
    let unrelated = current_ref!(fixture.editor).1.text().clone();
    let unrelated_selection = current_ref!(fixture.editor)
        .1
        .selection(fixture.editor.tree.focus)
        .clone();
    let doc = fixture.editor.document(id).unwrap();
    let transaction = Transaction::change(
        doc.text(),
        [(0, 2, Some("SSÉ".into())), (2, 3, Some("XX".into()))].into_iter(),
    );
    assert!(fixture.editor.apply_document_transaction(id, &transaction));
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "SSÉXX\n"
    );
    assert_eq!(current_ref!(fixture.editor).1.text(), &unrelated);
    assert_eq!(
        current_ref!(fixture.editor)
            .1
            .selection(fixture.editor.tree.focus),
        &unrelated_selection
    );
    fixture.editor.switch(id, Action::Replace);
    let (view, doc) = current!(fixture.editor);
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "ßéx\n");
    assert!(!doc.undo(view));
    assert!(doc.redo(view));
    assert_eq!(doc.text().to_string(), "SSÉXX\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_edits_preserve_pending_native_undo_groups_and_map_every_split(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let id = current_ref!(fixture.editor).1.id();
    let first = current_ref!(fixture.editor).0.id;
    fixture.editor.switch(id, Action::VerticalSplit);
    let second = current_ref!(fixture.editor).0.id;
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::point(2));
    let native = Transaction::change(doc.text(), [(0, 0, Some("N".into()))].into_iter());
    assert!(doc.apply(&native, view.id));
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("D".into()))].into_iter());
    assert!(fixture.editor.apply_document_transaction(id, &transaction));
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.text().to_string(), "DNabc\n");
    assert_eq!(doc.selection(second), &Selection::single(4, 5));
    assert_eq!(doc.selection(first), &Selection::single(2, 3));
    let (view, doc) = current!(fixture.editor);
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert_eq!(doc.selection(view.id), &Selection::single(2, 3));
    assert!(doc.redo(view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(doc.redo(view));
    assert_eq!(doc.text().to_string(), "DNabc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_only_edits_reject_selection_transactions_and_closed_documents(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let id = current_ref!(fixture.editor).1.id();
    let transaction = Transaction::change(
        fixture.editor.document(id).unwrap().text(),
        [(0, 1, Some("Z".into()))].into_iter(),
    )
    .with_selection(Selection::point(0));
    assert!(!fixture.editor.apply_document_transaction(id, &transaction));
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "abc\n"
    );
    let transaction = Transaction::change(
        fixture.editor.document(id).unwrap().text(),
        [(0, 1, Some("Z".into()))].into_iter(),
    );
    fixture.close_current()?;
    assert!(!fixture.editor.apply_document_transaction(id, &transaction));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_transactions_preserve_pending_native_undo_groups() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    let id = doc.id();
    let stale = Transaction::change(doc.text(), [(0, 1, Some("Z".into()))].into_iter());
    let first = Transaction::change(doc.text(), [(0, 0, Some("A".into()))].into_iter());
    assert!(doc.apply(&first, view.id));
    assert!(!fixture.editor.apply_document_transaction(id, &stale));
    let (view, doc) = current!(fixture.editor);
    let second = Transaction::change(doc.text(), [(0, 0, Some("B".into()))].into_iter());
    assert!(doc.apply(&second, view.id));
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert!(!doc.undo(view));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_transactions_preserve_pending_native_undo_groups() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    let id = doc.id();
    let first = Transaction::change(doc.text(), [(0, 0, Some("A".into()))].into_iter());
    assert!(doc.apply(&first, view.id));
    let empty = Transaction::new(doc.text());
    let version = doc.version();
    assert!(fixture.editor.apply_document_transaction(id, &empty));
    let (view, doc) = current!(fixture.editor);
    assert_eq!(doc.version(), version);
    let second = Transaction::change(doc.text(), [(0, 0, Some("B".into()))].into_iter());
    assert!(doc.apply(&second, view.id));
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert!(!doc.undo(view));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_originating_split_preserves_pending_edit_undo_selection() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new("abc\n")?;
    let id = current_ref!(fixture.editor).1.id();
    fixture.editor.switch(id, Action::VerticalSplit);
    let (view, doc) = current!(fixture.editor);
    let origin = view.id;
    doc.set_selection(origin, Selection::single(2, 3));
    let native = Transaction::change(doc.text(), [(0, 0, Some("N".into()))].into_iter());
    assert!(doc.apply(&native, origin));
    fixture.editor.close(origin);

    let doc = fixture.editor.document(id).unwrap();
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("D".into()))].into_iter());
    assert!(fixture.editor.apply_document_transaction(id, &transaction));
    let (view, doc) = current!(fixture.editor);
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert_eq!(doc.selection(view.id), &Selection::single(2, 3));
    assert!(doc.redo(view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(doc.redo(view));
    assert_eq!(doc.text().to_string(), "DNabc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_edits_synchronize_stale_split_jump_lists() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    let id = doc.id();
    let first = view.id;
    view.push_jump(doc, (id, Selection::single(2, 3)));
    fixture.editor.switch(id, Action::VerticalSplit);
    let (view, doc) = current!(fixture.editor);
    let second = view.id;
    view.push_jump(doc, (id, Selection::single(2, 3)));
    let committed = Transaction::change(doc.text(), [(0, 0, Some("C".into()))].into_iter());
    assert!(doc.apply(&committed, view.id));
    doc.append_changes_to_history(view);
    let pending = Transaction::change(doc.text(), [(0, 0, Some("P".into()))].into_iter());
    assert!(doc.apply(&pending, view.id));
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("D".into()))].into_iter());
    assert!(fixture.editor.apply_document_transaction(id, &transaction));

    for view_id in [first, second] {
        let view = fixture.editor.tree.get(view_id);
        let (_, selection) = view.jumps.iter().find(|(doc, _)| *doc == id).unwrap();
        assert_eq!(selection, &Selection::single(5, 6));
    }
    Ok(())
}
