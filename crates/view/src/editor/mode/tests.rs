use super::*;
use crate::{
    editor::{tests::editor, Action},
    Document,
};
use editor_core::{LineEnding, Rope, Selection};

#[tokio::test]
async fn append_at_eof_restores_graphemes_and_keeps_typing_in_one_undo_group() {
    for ending in [LineEnding::LF, LineEnding::Crlf] {
        let (mut editor, _store) = editor("#[|👩‍💻]#");
        let (view, doc) = current!(editor);
        doc.line_ending = ending;
        let original = doc.selection(view.id).clone();
        editor.enter_append_mode().unwrap();
        assert_eq!(editor.mode(), Mode::Insert);
        let (view, doc) = current!(editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            3
        );
        editing::insert::insert_char(doc, view.id, '界', None).unwrap();
        editor.enter_normal_mode();
        let (view, doc) = current!(editor);
        assert_eq!(doc.text(), format!("👩‍💻界{}", ending.as_str()).as_str());
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            3
        );
        doc.commit_changes();
        view.sync_changes(doc);
        assert!(editing::undo(doc, view.id));
        assert_eq!(doc.text(), "👩‍💻");
        assert_eq!(doc.selection(view.id), &original);
    }
}

#[tokio::test]
async fn rejected_mode_entry_preserves_mode_selection_and_undo() {
    let (mut editor, _store) = editor("#[|界]#");
    let doc = Document::from(
        Rope::from("界"),
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    )
    .with_readonly_name("adapter source".into());
    editor.new_file_from_document(Action::Replace, doc);
    editor.enter_select_mode();
    let (view, doc) = current_ref!(editor);
    let selection = doc.selection(view.id).clone();
    assert_eq!(editor.enter_append_mode(), Err(EditError::ReadOnlyDocument));
    assert_eq!(
        editor.open_lines(Open::Above, CommentContinuation::Enabled, 1),
        Err(EditError::ReadOnlyDocument)
    );
    assert_eq!(editor.mode(), Mode::Select);
    let (view, doc) = current!(editor);
    assert_eq!(doc.text(), "界");
    assert_eq!(doc.selection(view.id), &selection);
    assert!(!editing::undo(doc, view.id));
}

#[tokio::test]
async fn focus_and_buffer_changes_finish_append_on_the_origin_view() {
    let (mut editor, _store) = editor("#[é|]#界\n");
    let first = editor.tree.focus();
    let document = current_ref!(editor).1.id();
    editor.switch(document, Action::VerticalSplit);
    let second = editor.tree.focus();
    let second_selection = current_ref!(editor).1.selection(second).clone();
    editor.focus(first);
    let original = current_ref!(editor).1.selection(first).clone();
    editor.enter_append_mode().unwrap();
    editor.focus(second);
    assert_eq!(editor.mode(), Mode::Normal);
    let doc = current_ref!(editor).1;
    assert_eq!(doc.selection(first), &original);
    assert_eq!(doc.selection(second), &second_selection);
    editor.enter_insert_mode();
    editor.enter_normal_mode();
    assert_eq!(current_ref!(editor).1.selection(first), &original);

    editor.focus(first);
    editor.enter_append_mode().unwrap();
    let new_document = editor.new_file(Action::Replace);
    assert_eq!(editor.mode(), Mode::Normal);
    editor.switch(document, Action::Replace);
    assert_eq!(current_ref!(editor).1.selection(first), &original);
    assert_ne!(new_document, document);
}

#[tokio::test]
async fn closing_append_view_cannot_restore_another_views_cursor() {
    let (mut editor, _store) = editor("#[é|]#界\n");
    let first = editor.tree.focus();
    let doc_id = current_ref!(editor).1.id();
    editor.switch(doc_id, Action::VerticalSplit);
    let second = editor.tree.focus();
    editor.focus(first);
    editor.enter_append_mode().unwrap();
    let other_selection = editor.document(doc_id).unwrap().selection(second).clone();
    editor.close(first);
    editor.enter_normal_mode();
    assert_eq!(
        editor.document(doc_id).unwrap().selection(second),
        &other_selection
    );
    assert!(editor
        .document(doc_id)
        .unwrap()
        .try_selection(first)
        .is_none());
}

#[tokio::test]
async fn insert_entry_discards_append_restoration_and_select_entry_handles_eof() {
    let (mut editor, _store) = editor("#[|é界]#\n");
    editor.enter_append_mode().unwrap();
    editor.enter_insert_mode();
    let selection = current_ref!(editor)
        .1
        .selection(editor.tree.focus())
        .clone();
    editor.enter_normal_mode();
    assert_eq!(
        current_ref!(editor).1.selection(editor.tree.focus()),
        &selection
    );

    let (view, doc) = current!(editor);
    doc.set_selection(view.id, Selection::point(doc.text().len_chars()));
    editor.enter_select_mode();
    let (view, doc) = current_ref!(editor);
    assert_eq!(doc.selection(view.id).primary(), Range::new(2, 3));
}

#[tokio::test]
async fn line_boundary_and_open_line_entry_work_with_both_line_endings() {
    for ending in [LineEnding::LF, LineEnding::Crlf] {
        let (mut editor, _store) = editor("  #[|é]#\n");
        current!(editor).1.line_ending = ending;
        editor
            .insert_at_line_boundary(LineBoundary::LineStart)
            .unwrap();
        assert_eq!(editor.mode(), Mode::Insert);
        let (view, doc) = current_ref!(editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            2
        );
        editor.enter_normal_mode();
        editor
            .open_lines(Open::Above, CommentContinuation::Disabled, 2)
            .unwrap();
        assert_eq!(editor.mode(), Mode::Insert);
        let (view, doc) = current_ref!(editor);
        assert_eq!(doc.selection(view.id).len(), 2);
        assert!(doc.text().to_string().starts_with(ending.as_str()));
    }
}
#[tokio::test]
async fn leaving_insert_mode_removes_new_blank_line_indentation_in_character_coordinates() {
    for ending in [LineEnding::LF, LineEnding::Crlf] {
        let newline = ending.as_str();
        let (mut editor, _store) = editor(&format!("a#[|{newline}]#"));
        editor.enter_insert_mode();
        let (view, doc) = current!(editor);
        let inserted = format!("{newline}\u{2003}");
        let cursor = 1 + inserted.chars().count();
        let transaction =
            Transaction::change(doc.text(), [(1, 1, Some(inserted.into()))].into_iter())
                .with_selection(Selection::point(cursor));
        editing::apply(doc, &transaction, view.id).unwrap();
        editor.enter_normal_mode();
        let (view, doc) = current_ref!(editor);
        assert_eq!(doc.text(), format!("a{newline}{newline}").as_str());
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            1 + ending.len_chars()
        );
    }
}

#[tokio::test]
async fn debug_mode_survives_source_navigation_and_finishes_insert_state() {
    let (mut editor, _store) = editor("#[é|]#界\n");
    let first = editor.tree.focus();
    let doc_id = current_ref!(editor).1.id();
    let original = current_ref!(editor).1.selection(first).clone();
    editor.enter_append_mode().unwrap();
    editor.enter_debug_mode();
    assert_eq!(editor.mode(), Mode::Debug);
    assert_eq!(current_ref!(editor).1.selection(first), &original);
    editor.switch(doc_id, Action::VerticalSplit);
    assert_eq!(editor.mode(), Mode::Debug);
    editor.focus(first);
    assert_eq!(editor.mode(), Mode::Debug);
    editor.new_file(Action::Replace);
    assert_eq!(editor.mode(), Mode::Debug);
    editor.enter_insert_mode();
    editor.enter_normal_mode();
    assert_eq!(editor.mode(), Mode::Normal);
}

#[tokio::test]
async fn temporary_modes_restore_command_mode_and_finish_the_insert_transaction() {
    for command_mode in [Mode::Normal, Mode::Debug] {
        let (mut editor, _store) = editor("#[|é]#界\n");
        editor.set_mode(command_mode);
        editor.enter_append_mode().unwrap();
        let (view, doc) = current!(editor);
        editing::insert::insert_char(doc, view.id, '語', None).unwrap();
        editor.enter_select_mode();
        editor.leave_editing_mode();
        assert_eq!(editor.mode(), command_mode);
        let (view, doc) = current!(editor);
        assert_eq!(doc.text(), "é語界\n");
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            1
        );
        doc.commit_changes();
        view.sync_changes(doc);
        assert!(editing::undo(doc, view.id));
        assert_eq!(doc.text(), "é界\n");

        editor.enter_insert_mode();
        editor.enter_normal_mode();
        editor.enter_select_mode();
        editor.leave_editing_mode();
        assert_eq!(editor.mode(), Mode::Normal);
    }
}

#[tokio::test]
async fn changing_views_or_documents_during_an_edit_restores_debug_mode() {
    let (mut editor, _store) = editor("#[é|]#界\n");
    let first = editor.tree.focus();
    let document = current_ref!(editor).1.id();
    editor.switch(document, Action::VerticalSplit);
    let second = editor.tree.focus();
    editor.focus(first);
    let original = current_ref!(editor).1.selection(first).clone();
    editor.enter_debug_mode();
    editor.enter_append_mode().unwrap();
    editor.focus(second);
    assert_eq!(editor.mode(), Mode::Debug);
    assert_eq!(
        editor.document(document).unwrap().selection(first),
        &original
    );
    editor.enter_select_mode();
    editor.new_file(Action::Replace);
    assert_eq!(editor.mode(), Mode::Debug);
    editor.enter_insert_mode();
    editor.switch(document, Action::Replace);
    assert_eq!(editor.mode(), Mode::Debug);
}
