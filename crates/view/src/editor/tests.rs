//! Headless editor fixture for editor operation tests.
use crate::{
    callbacks::EditorCallbackSender, editor::Action, graphics::Rect, handlers::Handlers, Document,
    Editor,
};
use arc_swap::ArcSwap;
use editor_config::Config;
use editor_core::{test, LineEnding, Rope};
use std::sync::Arc;

pub(super) fn editor(input: &str) -> (Editor, tempfile::TempDir) {
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
