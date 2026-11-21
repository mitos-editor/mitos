//! Headless editor fixture for editor operation tests.
use crate::{
    callbacks::EditorCallbackSender, editor::Action, graphics::Rect, handlers::Handlers, Document,
    Editor,
};
use arc_swap::ArcSwap;
use editor_config::Config;
use editor_core::{test, LineEnding, Rope};
use std::sync::Arc;

pub(crate) fn editor(input: &str) -> (Editor, tempfile::TempDir) {
    let store = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.files.watch.enable = false;
    config.files.reload.enable = false;
    config.files.reload.poll.enable = false;
    config.lsp.enable = false;
    config.completion.auto_trigger = false;
    config.completion.path.enable = false;
    let callbacks = EditorCallbackSender::new(Default::default(), |_| async {}, |_| {});
    let handlers = Handlers::new(&config, callbacks);
    let config = Arc::new(ArcSwap::from_pointee(config));
    let loader = Arc::new(ArcSwap::from_pointee(language::syntax::Loader::default()));
    let mut editor = Editor::new(
        Rect::new(0, 0, 80, 24),
        Arc::new(theme::Loader::new(loader::theme::Resources::new(vec![]))),
        loader,
        config,
        handlers,
        workspace::trust::WorkspaceTrust::fully_trusted(store.path().join("trust.toml")),
    );
    let (text, selection) = test::print(input);
    let mut doc = Document::from(
        Rope::from(text),
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    );
    doc.line_ending = LineEnding::LF;
    let id = editor.new_file_from_document(Action::VerticalSplit, doc);
    let view = editor.tree.focus();
    editor
        .document_mut(id)
        .unwrap()
        .set_selection(view, selection);
    (editor, store)
}

#[tokio::test]
async fn closed_buffers_leave_the_last_modified_history() {
    let (mut editor, _store) = editor("#[|a]#bc\n");
    let older = doc!(editor).id();
    editor.insert_char('a').unwrap();
    let newer = editor.new_file(Action::Replace);
    editor.insert_char('b').unwrap();
    let current = editor.new_file(Action::Replace);

    assert!(editor.close_document(newer, true).is_ok());
    assert_eq!(view!(editor).last_modified_document(), Some(older));

    // Recording another modification must preserve the surviving older entry.
    editor.insert_char('c').unwrap();
    editor.switch(older, Action::Replace);
    assert!(editor.close_document(current, true).is_ok());
    editor.new_file(Action::Replace);
    assert_eq!(view!(editor).last_modified_document(), Some(older));
}

#[tokio::test]
async fn jumping_in_an_unfocused_split_restores_its_selection() {
    let (mut editor, _store) = editor("ab#[|c]#def\n");
    editor.insert_char('x').unwrap();
    let (view, doc) = current_ref!(editor);
    let jumped_view = view.id;
    let destination = doc.id();
    let selection = doc.selection(jumped_view).clone();
    editor.new_file(Action::Replace);
    let other_doc = editor.new_file(Action::VerticalSplit);
    let focused_view = editor.tree.focus();
    editor
        .document_mut(destination)
        .unwrap()
        .set_selection(jumped_view, editor_core::Selection::point(0));

    editor.jump_backward(jumped_view, 1);

    assert_eq!(editor.tree.focus(), focused_view);
    assert_eq!(view!(editor, jumped_view).document_id(), destination);
    assert_eq!(
        editor.document(destination).unwrap().selection(jumped_view),
        &selection
    );
    assert!(editor
        .document(other_doc)
        .unwrap()
        .try_selection(jumped_view)
        .is_none());
}
