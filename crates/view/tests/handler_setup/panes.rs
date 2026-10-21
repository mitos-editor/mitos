use super::Fixture;
use editor_core::{
    movement::{Direction, Movement},
    Selection,
};
use event::register_hook;
use std::sync::{Arc, Mutex};
use view::{
    current_ref,
    editor::Action,
    events::{DocumentFocusLost, SelectionDidChange},
    view::ViewPosition,
};

#[tokio::test(flavor = "multi_thread")]
async fn same_document_splits_publish_the_copied_selection_and_viewport() -> anyhow::Result<()> {
    let text = "line\n".repeat(80);
    for action in [Action::HorizontalSplit, Action::VerticalSplit] {
        let mut f = Fixture::new(&text)?;
        let (source, doc) = current_ref!(f.editor);
        let (source, id) = (source.id, doc.id());
        let selection = Selection::single(175, 180);
        let offset = ViewPosition {
            anchor: 150,
            ..ViewPosition::default()
        };
        let doc = f.editor.document_mut(id).unwrap();
        doc.set_selection(source, selection.clone());
        doc.set_view_offset(source, offset);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = observed.clone();
        register_hook!(move |event: &mut SelectionDidChange<'_>| {
            if event.doc.id() == id && event.view != source {
                captured.lock().unwrap().push((
                    event.doc.selection(event.view).clone(),
                    event.doc.view_offset(event.view),
                ));
            }
            Ok(())
        });
        f.editor.switch(id, action);
        let (split, doc) = current_ref!(f.editor);
        assert_ne!(split.id, source);
        assert_eq!(doc.selection(split.id), &selection);
        assert_eq!(doc.view_offset(split.id), offset);
        assert_eq!(*observed.lock().unwrap(), vec![(selection.clone(), offset)]);
        assert_eq!(doc.selection(source), &selection);
        assert_eq!(doc.text(), text.as_str());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scrolling_an_unfocused_split_preserves_focus_mode_and_other_views() -> anyhow::Result<()> {
    use view::editing::movement::ScrollCursor;
    let text = "line\n".repeat(80);
    let mut f = Fixture::new(&text)?;
    let (first, doc) = current_ref!(f.editor);
    let (first, id) = (first.id, doc.id());
    f.editor.switch(id, Action::VerticalSplit);
    let focused = f.editor.tree.focus();
    f.editor.enter_insert_mode();
    let mode = f.editor.mode();
    let doc = f.editor.document(id).unwrap();
    let selection = doc.selection(focused).clone();
    let offset = doc.view_offset(focused);
    let version = doc.version();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        captured.lock().unwrap().push(event.view);
        Ok(())
    });
    f.editor.scroll_view(
        first,
        5,
        Direction::Forward,
        ScrollCursor::KeepVisible,
        Movement::Move,
    );
    assert_eq!(f.editor.tree.focus(), focused);
    assert_eq!(f.editor.mode(), mode);
    assert!(events.lock().unwrap().is_empty());
    let doc = f.editor.document(id).unwrap();
    assert!(doc.view_offset(first).anchor > 0);
    assert!(doc.selection(first).primary().head > 0);
    assert_eq!(doc.selection(focused), &selection);
    assert_eq!(doc.view_offset(focused), offset);
    assert_eq!(doc.version(), version);
    assert_eq!(doc.text(), text.as_str());
    f.editor.close(first);
    f.editor.scroll_view(
        first,
        5,
        Direction::Forward,
        ScrollCursor::Follow,
        Movement::Move,
    );
    assert_eq!(f.editor.tree.focus(), focused);
    Ok(())
}
