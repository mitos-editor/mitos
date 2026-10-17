use super::Fixture;
use editor_core::{Selection, Transaction};
use view::{current, current_ref, editor::Action};

#[tokio::test(flavor = "multi_thread")]
async fn reordering_preserves_primary_content_and_can_be_undone() -> anyhow::Result<()> {
    use editor_core::{transform::ReorderStrategy, Range};

    let mut fixture = Fixture::new("a bb ccc\n")?;
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(
        view.id,
        Selection::new(
            vec![Range::new(0, 1), Range::new(2, 4), Range::new(5, 8)].into(),
            0,
        ),
    );
    view::editing::reorder_selection_contents(doc, view.id, ReorderStrategy::RotateForward, 1);
    assert_eq!(doc.text().to_string(), "ccc a bb\n");
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .slice(doc.text().slice(..))
            .to_string(),
        "a"
    );

    view::editing::commit(doc, view);
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "a bb ccc\n");
    assert!(view::editing::redo(doc, view));
    assert_eq!(doc.text().to_string(), "ccc a bb\n");
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .slice(doc.text().slice(..))
            .to_string(),
        "a"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn same_length_stale_edits_preserve_pending_typing() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    let (id, version) = (doc.id(), doc.version());
    let stale = Transaction::change(doc.text(), [(1, 2, Some("Z".into()))].into_iter());
    let first = Transaction::change(doc.text(), [(0, 1, Some("A".into()))].into_iter());
    assert!(view::editing::apply(doc, &first, view.id));
    let actual = doc.version();
    assert_eq!(
        fixture
            .editor
            .apply_document_changes(id, version, stale.changes()),
        Err(view::editing::EditError::DocumentChanged {
            expected: version,
            actual
        }),
    );
    let (view, doc) = current!(fixture.editor);
    assert_eq!(doc.text().to_string(), "Abc\n");
    let second = Transaction::change(doc.text(), [(2, 3, Some("C".into()))].into_iter());
    assert!(view::editing::apply(doc, &second, view.id));
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert!(!view::editing::undo(doc, view));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_edits_and_reload_do_not_create_hidden_buffer_selections() -> anyhow::Result<()> {
    use lsp_client::{lsp, OffsetEncoding};
    let mut fixture = Fixture::new("focused\n")?;
    let path = fixture.dir.path().join("hidden.words");
    std::fs::write(&path, "abc\n")?;
    let id = fixture.editor.open(&path, Action::Load)?;
    let focused = current_ref!(fixture.editor).1.id();
    assert!(fixture
        .editor
        .document(id)
        .unwrap()
        .selections()
        .next()
        .is_none());
    let edit = lsp::WorkspaceEdit {
        changes: Some(
            [(
                lsp_client::Url::from_file_path(&path).unwrap(),
                vec![lsp::TextEdit {
                    range: lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 1)),
                    new_text: "A".into(),
                }],
            )]
            .into(),
        ),
        ..Default::default()
    };
    fixture
        .editor
        .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
        .unwrap();
    let doc = fixture.editor.document_mut(id).unwrap();
    assert_eq!(doc.text(), "Abc\n");
    assert!(doc.undo());
    assert_eq!(doc.text(), "abc\n");
    assert!(doc.redo());
    std::fs::write(&path, "disk\n")?;
    fixture.editor.reload_document(id)?;
    let doc = fixture.editor.document_mut(id).unwrap();
    assert_eq!(doc.text(), "disk\n");
    assert!(doc.selections().next().is_none());
    assert!(doc.undo());
    assert_eq!(doc.text(), "Abc\n");
    assert_eq!(current_ref!(fixture.editor).1.id(), focused);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn save_all_normalizes_hidden_buffers_without_creating_selections() -> anyhow::Result<()> {
    use view::save::{save_all, WriteAllOptions};
    let mut fixture = Fixture::new("focused\n")?;
    fixture.configure(|config| config.files.save.trim_trailing_whitespace = true);
    let path = fixture.dir.path().join("hidden.words");
    std::fs::write(&path, "abc\n")?;
    let id = fixture.editor.open(&path, Action::Load)?;
    let doc = fixture.editor.document_mut(id).unwrap();
    let edit = Transaction::change(doc.text(), [(3, 4, Some("  ".into()))].into_iter());
    doc.apply(edit.changes())?;
    doc.commit_changes();
    save_all(
        &mut fixture.editor,
        WriteAllOptions {
            force: false,
            write_scratch: false,
            auto_format: false,
            code_actions: false,
        },
        |editor, request| {
            assert!(request.origin.is_none());
            editor.save(request.doc_id, request.path, request.force)
        },
    )?;
    fixture.editor.flush_writes().await?;
    let doc = fixture.editor.document_mut(id).unwrap();
    assert_eq!(std::fs::read_to_string(path)?, "abc\n");
    assert!(doc.selections().next().is_none());
    assert!(doc.undo());
    assert_eq!(doc.text(), "abc  ");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn branching_history_restores_each_revisions_selection() -> anyhow::Result<()> {
    use editor_core::history::UndoKind;
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::single(0, 1));
    let first = Transaction::change(doc.text(), [(0, 1, Some("A".into()))].into_iter())
        .with_selection(Selection::single(1, 2));
    assert!(view::editing::apply(doc, &first, view.id));
    view::editing::commit(doc, view);
    let second = Transaction::change(doc.text(), [(1, 2, Some("B".into()))].into_iter())
        .with_selection(Selection::single(2, 3));
    assert!(view::editing::apply(doc, &second, view.id));
    view::editing::commit(doc, view);
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.selection(view.id), &Selection::single(1, 2));
    let alternative = Transaction::change(doc.text(), [(1, 2, Some("Z".into()))].into_iter())
        .with_selection(Selection::single(0, 1));
    assert!(view::editing::apply(doc, &alternative, view.id));
    view::editing::commit(doc, view);
    assert!(view::editing::earlier(doc, view, UndoKind::Steps(1)));
    assert_eq!(doc.text().to_string(), "ABc\n");
    assert_eq!(doc.selection(view.id), &Selection::single(2, 3));
    assert!(view::editing::later(doc, view, UndoKind::Steps(1)));
    assert_eq!(doc.text().to_string(), "AZc\n");
    assert_eq!(doc.selection(view.id), &Selection::single(0, 1));
    Ok(())
}

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
    fixture.editor.apply_document_changes(
        id,
        fixture.editor.document(id).unwrap().version(),
        transaction.changes(),
    )?;
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
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "ßéx\n");
    assert!(!view::editing::undo(doc, view));
    assert!(view::editing::redo(doc, view));
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
    assert!(view::editing::apply(doc, &native, view.id));
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("D".into()))].into_iter());
    fixture.editor.apply_document_changes(
        id,
        fixture.editor.document(id).unwrap().version(),
        transaction.changes(),
    )?;
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.text().to_string(), "DNabc\n");
    assert_eq!(doc.selection(second), &Selection::single(4, 5));
    assert_eq!(doc.selection(first), &Selection::single(2, 3));
    let (view, doc) = current!(fixture.editor);
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert_eq!(doc.selection(view.id), &Selection::single(2, 3));
    assert!(view::editing::redo(doc, view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(view::editing::redo(doc, view));
    assert_eq!(doc.text().to_string(), "DNabc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_document_edits_are_rejected() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let doc = current_ref!(fixture.editor).1;
    let (id, version) = (doc.id(), doc.version());
    let transaction = Transaction::change(doc.text(), [(0, 1, Some("Z".into()))].into_iter());
    fixture.close_current()?;
    assert_eq!(
        fixture
            .editor
            .apply_document_changes(id, version, transaction.changes()),
        Err(view::editing::EditError::DocumentClosed(id))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_transactions_preserve_pending_native_undo_groups() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    let id = doc.id();
    let stale = Transaction::change(doc.text(), [(0, 1, Some("Z".into()))].into_iter());
    let first = Transaction::change(doc.text(), [(0, 0, Some("A".into()))].into_iter());
    assert!(view::editing::apply(doc, &first, view.id));
    assert!(fixture
        .editor
        .apply_document_changes(
            id,
            fixture.editor.document(id).unwrap().version(),
            stale.changes()
        )
        .is_err());
    let (view, doc) = current!(fixture.editor);
    let second = Transaction::change(doc.text(), [(0, 0, Some("B".into()))].into_iter());
    assert!(view::editing::apply(doc, &second, view.id));
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert!(!view::editing::undo(doc, view));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_transactions_preserve_pending_native_undo_groups() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    let id = doc.id();
    let first = Transaction::change(doc.text(), [(0, 0, Some("A".into()))].into_iter());
    assert!(view::editing::apply(doc, &first, view.id));
    let empty = Transaction::new(doc.text());
    let version = doc.version();
    fixture.editor.apply_document_changes(
        id,
        fixture.editor.document(id).unwrap().version(),
        empty.changes(),
    )?;
    let (view, doc) = current!(fixture.editor);
    assert_eq!(doc.version(), version);
    let second = Transaction::change(doc.text(), [(0, 0, Some("B".into()))].into_iter());
    assert!(view::editing::apply(doc, &second, view.id));
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert!(!view::editing::undo(doc, view));
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
    assert!(view::editing::apply(doc, &native, origin));
    fixture.editor.close(origin);

    let doc = fixture.editor.document(id).unwrap();
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("D".into()))].into_iter());
    fixture.editor.apply_document_changes(
        id,
        fixture.editor.document(id).unwrap().version(),
        transaction.changes(),
    )?;
    let (view, doc) = current!(fixture.editor);
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "abc\n");
    assert_eq!(doc.selection(view.id), &Selection::single(2, 3));
    assert!(view::editing::redo(doc, view));
    assert_eq!(doc.text().to_string(), "Nabc\n");
    assert!(view::editing::redo(doc, view));
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
    assert!(view::editing::apply(doc, &committed, view.id));
    view::editing::commit(doc, view);
    let pending = Transaction::change(doc.text(), [(0, 0, Some("P".into()))].into_iter());
    assert!(view::editing::apply(doc, &pending, view.id));
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("D".into()))].into_iter());
    fixture.editor.apply_document_changes(
        id,
        fixture.editor.document(id).unwrap().version(),
        transaction.changes(),
    )?;

    for view_id in [first, second] {
        let view = fixture.editor.tree.get(view_id);
        let (_, selection) = view.jumps.iter().find(|(doc, _)| *doc == id).unwrap();
        assert_eq!(selection, &Selection::single(5, 6));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_undo_groups_do_not_supply_cursors_to_later_document_edits() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::single(1, 2));
    let insert = Transaction::change(doc.text(), [(0, 0, Some("x".into()))].into_iter());
    assert!(view::editing::apply(doc, &insert, view.id));
    let remove = Transaction::change(doc.text(), [(0, 1, None)].into_iter());
    doc.apply(remove.changes())?;
    assert_eq!(doc.commit_changes(), None);
    let edit = Transaction::change(doc.text(), [(0, 1, Some("A".into()))].into_iter());
    doc.apply(edit.changes())?;
    doc.commit_changes();
    doc.set_selection(view.id, Selection::single(2, 3));
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text(), "abc\n");
    assert_eq!(doc.selection(view.id), &Selection::single(2, 3));
    Ok(())
}
