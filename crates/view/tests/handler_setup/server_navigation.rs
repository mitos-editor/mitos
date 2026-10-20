use super::Fixture;
use editor_core::{Selection, Uri};
use lsp_client::{lsp, OffsetEncoding};
use view::{current_ref, editor::Action};

fn selection(start: u32, end: u32) -> lsp::Range {
    lsp::Range::new(lsp::Position::new(0, start), lsp::Position::new(0, end))
}

#[tokio::test]
async fn document_locations_decode_the_servers_encoding_and_focus_the_destination(
) -> anyhow::Result<()> {
    for (encoding, start, end) in [
        (OffsetEncoding::Utf8, 5, 11),
        (OffsetEncoding::Utf16, 3, 9),
        (OffsetEncoding::Utf32, 2, 8),
    ] {
        let mut f = Fixture::new("origin\n")?;
        let path = f.dir.path().join("target.words");
        std::fs::write(&path, "😀 target\n")?;
        let id = f.editor.open_document_location(
            &Uri::from(path),
            Some(selection(start, end)),
            encoding,
            Action::Replace,
        )?;
        let (view, doc) = current_ref!(f.editor);
        assert_eq!(doc.id(), id);
        assert_eq!(doc.selection(view.id), &Selection::single(8, 2));
    }
    Ok(())
}

#[tokio::test]
async fn background_loads_do_not_create_selections_for_unrelated_views() -> anyhow::Result<()> {
    let mut f = Fixture::new("origin\n")?;
    let (view, doc) = current_ref!(f.editor);
    let (origin, document, before) = (view.id, doc.id(), doc.selection(view.id).clone());
    let path = f.dir.path().join("target.words");
    std::fs::write(&path, "target\n")?;
    let target = f.editor.open_document_location(
        &Uri::from(path),
        Some(selection(1, 4)),
        OffsetEncoding::Utf16,
        Action::Load,
    )?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!((view.id, doc.id()), (origin, document));
    assert_eq!(doc.selection(view.id), &before);
    assert_eq!(f.editor.document(target).unwrap().view_ids().count(), 0);
    Ok(())
}

#[tokio::test]
async fn background_locations_position_an_existing_destination_split_without_changing_focus(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("origin\n")?;
    let origin = current_ref!(f.editor).0.id;
    let path = f.dir.path().join("target.words");
    std::fs::write(&path, "target\n")?;
    let target = f.editor.open(&path, Action::VerticalSplit)?;
    let target_view = current_ref!(f.editor).0.id;
    f.editor.focus(origin);
    f.editor.open_document_location(
        &Uri::from(path),
        Some(selection(1, 4)),
        OffsetEncoding::Utf16,
        Action::Load,
    )?;
    assert_eq!(current_ref!(f.editor).0.id, origin);
    let doc = f.editor.document(target).unwrap();
    assert_eq!(doc.selection(target_view), &Selection::single(4, 1));
    assert!(doc.try_selection(origin).is_none());
    Ok(())
}

#[tokio::test]
async fn omitted_positions_preserve_selection_and_oversized_positions_clamp_to_the_document(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("original\n")?;
    let (view, doc) = current_ref!(f.editor);
    let (view_id, path, before) = (
        view.id,
        doc.path().unwrap().to_owned(),
        doc.selection(view.id).clone(),
    );
    f.editor.open_document_location(
        &Uri::from(path.clone()),
        None,
        OffsetEncoding::Utf16,
        Action::Replace,
    )?;
    assert_eq!(current_ref!(f.editor).1.selection(view_id), &before);
    f.editor.open_document_location(
        &Uri::from(path),
        Some(lsp::Range::new(
            lsp::Position::new(999, 0),
            lsp::Position::new(999, 1),
        )),
        OffsetEncoding::Utf16,
        Action::Replace,
    )?;
    let (_, doc) = current_ref!(f.editor);
    assert_eq!(
        doc.selection(view_id),
        &Selection::point(doc.text().len_chars())
    );
    Ok(())
}

#[tokio::test]
async fn hidden_locations_follow_edits_and_are_consumed_when_the_document_is_displayed(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("origin\n")?;
    let path = f.dir.path().join("target.words");
    std::fs::write(&path, "target\n")?;
    let target = f.editor.open_document_location(
        &Uri::from(path),
        Some(selection(1, 4)),
        OffsetEncoding::Utf16,
        Action::Load,
    )?;
    let doc = f.editor.document_mut(target).unwrap();
    let change =
        editor_core::Transaction::change(doc.text(), [(0, 0, Some("prefix ".into()))].into_iter());
    doc.apply(change.changes())?;
    assert_eq!(doc.view_ids().count(), 0);
    f.editor.switch(target, Action::Replace);
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.selection(view.id), &Selection::single(11, 8));
    // A later split initializes from normal view state, not the old request.
    let view_id = view.id;
    f.editor
        .document_mut(target)
        .unwrap()
        .set_selection(view_id, Selection::point(0));
    f.editor.switch(target, Action::VerticalSplit);
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.selection(view.id), &Selection::single(0, 1));
    Ok(())
}
