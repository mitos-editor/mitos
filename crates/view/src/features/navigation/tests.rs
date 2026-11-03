use super::*;
use crate::editor::{tests::editor, Action};
use editor_core::{Selection, Transaction};

fn snapshot(editor: &mut Editor) -> NavigationUpdate {
    let owner = Arc::downgrade(&editor.handlers.navigation.callbacks);
    let mode = editor.mode();
    let (view, doc) = crate::current!(editor);
    let cancel = doc
        .view_state_mut(view.id)
        .navigation
        .get_or_insert_default()
        .restart();
    let target = (doc.id(), view.id);
    let request = Request {
        owner,
        target: SelectionRequest::new(doc, view.id, cancel, vec![]),
        kind: NavigationKind::Definition,
        mode,
    };
    editor.handlers.navigation.target = Some(target);
    NavigationUpdate {
        request,
        locations: vec![Location {
            uri: editor_core::Uri::from(std::env::temp_dir().join("navigation-target")),
            range: lsp::Range::default(),
            offset_encoding: OffsetEncoding::Utf16,
        }],
    }
}

fn enabled_editor() -> (Editor, tempfile::TempDir) {
    let (mut editor, dir) = editor("#[|a]#bc\n");
    let mut config = (*editor.config()).clone();
    config.lsp.enable = true;
    editor.config = Arc::new(arc_swap::ArcSwap::from_pointee(config));
    (editor, dir)
}

#[tokio::test]
async fn snapshots_reject_selection_round_trips_and_reused_views() {
    let (mut editor, _dir) = enabled_editor();
    assert!(snapshot(&mut editor).resolve(&mut editor).is_some());
    let update = snapshot(&mut editor);
    let (view, doc) = crate::current!(editor);
    let original = doc.selection(view.id).clone();
    doc.set_selection(view.id, Selection::point(2));
    doc.set_selection(view.id, original);
    assert!(!update.request.is_current(&editor));
    assert!(update.resolve(&mut editor).is_none());

    let update = snapshot(&mut editor);
    let document = crate::current_ref!(editor).1.id();
    editor.new_file(Action::Replace);
    editor.switch(document, Action::Replace);
    assert!(!update.request.is_current(&editor));
    assert!(update.resolve(&mut editor).is_none());
}

#[tokio::test]
async fn snapshots_reject_renamed_or_closed_documents() {
    let (mut editor, dir) = enabled_editor();
    let update = snapshot(&mut editor);
    let document = crate::current_ref!(editor).1.id();
    editor.set_doc_path(document, &dir.path().join("renamed.words"));
    assert!(!update.request.is_current(&editor));
    assert!(update.resolve(&mut editor).is_none());
    let update = snapshot(&mut editor);
    editor.new_file(Action::Replace);
    assert!(editor.close_document(document, true).is_ok());
    assert!(!update.request.is_current(&editor));
    assert!(update.resolve(&mut editor).is_none());
}

#[tokio::test]
async fn snapshots_reject_restored_text_canceled_work_and_replaced_owners() {
    let (mut editor, _dir) = enabled_editor();
    let update = snapshot(&mut editor);
    let (view, doc) = crate::current!(editor);
    for text in ["xyz\n", "abc\n"] {
        let change = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        crate::editing::apply(doc, &change, view.id).unwrap();
    }
    assert!(update.resolve(&mut editor).is_none());

    let update = snapshot(&mut editor);
    cancel(&mut editor);
    assert!(update.resolve(&mut editor).is_none());
    let older = snapshot(&mut editor);
    let current = snapshot(&mut editor);
    assert!(older.resolve(&mut editor).is_none());
    assert!(current.resolve(&mut editor).is_some());

    let update = snapshot(&mut editor);
    editor.handlers.navigation = NavigationHandler::new(editor.callback_sender().clone());
    assert!(!update.request.is_current(&editor));
    assert!(update.resolve(&mut editor).is_none());
}
