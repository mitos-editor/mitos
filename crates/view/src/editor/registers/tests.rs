use crate::{
    callbacks::EditorCallbackSender, document::Mode, editing, editor::Action, graphics::Rect,
    handlers::Handlers, Document, Editor,
};
use arc_swap::ArcSwap;
use editor_config::Config;
use editor_core::{test, transform::PastePosition, LineEnding, Rope, Transaction};
use std::sync::Arc;

fn editor(input: &str) -> (Editor, tempfile::TempDir) {
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

fn assert_text(editor: &Editor, expected: &str) {
    let (view, doc) = current_ref!(editor);
    assert_eq!(
        test::plain(doc.text().slice(..), doc.selection(view.id)),
        expected
    );
}

#[tokio::test]
async fn yanks_resolve_selection_order_primary_and_joined_values() {
    let (mut editor, _store) = editor("#(é|)# #[|界]#\n");
    assert_eq!(editor.yank_selections('a').unwrap(), 2);
    let values: Vec<_> = editor
        .read_register('a')
        .unwrap()
        .map(|v| v.into_owned())
        .collect();
    assert_eq!(values, ["é", "界"]);
    editor.yank_primary_selection('b').unwrap();
    assert_eq!(editor.first_register_value('b').as_deref(), Some("界"));
    assert_eq!(editor.yank_joined_selections('c', "\r\n").unwrap(), 2);
    assert_eq!(editor.first_register_value('c').as_deref(), Some("é\r\n界"));
}

#[tokio::test]
async fn failed_register_writes_leave_text_selections_mode_and_history_alone() {
    for register in ['#', '+'] {
        let (mut editor, _store) = editor("#[|abc]#\n");
        editor
            .registers
            .set_clipboard_backend(Box::new(FailingClipboard));
        editor.set_mode(Mode::Select);
        assert!(editor.change_selections(Some(register), 1).is_err());
        assert_eq!(editor.mode(), Mode::Select);
        assert_text(&editor, "#[|abc]#\n");
        let (view, doc) = current!(editor);
        assert!(!editing::undo(doc, view));
    }
}

#[tokio::test]
async fn rejected_edits_do_not_overwrite_registers_or_change_mode() {
    let (mut editor, _store) = editor("#[|abc]#\n");
    let (view, doc) = current_ref!(editor);
    let view_id = view.id;
    let (text, selection) = (doc.text().clone(), doc.selection(view_id).clone());
    let readonly = Document::from(
        text,
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    )
    .with_readonly_name("adapter source".into());
    let id = editor.new_file_from_document(Action::Replace, readonly);
    editor
        .document_mut(id)
        .unwrap()
        .set_selection(view_id, selection);
    editor.set_mode(Mode::Select);
    editor.registers.write('a', vec!["saved".into()]).unwrap();
    assert!(editor.delete_selections(Some('a')).is_err());
    assert!(editor.change_selections(Some('a'), 1).is_err());
    assert_eq!(editor.first_register_value('a').as_deref(), Some("saved"));
    assert_eq!(editor.mode(), Mode::Select);
    assert_text(&editor, "#[|abc]#\n");
    let (view, doc) = current!(editor);
    assert!(!editing::undo(doc, view));
}

#[tokio::test]
async fn linewise_change_and_its_yank_share_one_undo_group() {
    for ending in [LineEnding::LF, LineEnding::Crlf] {
        let newline = ending.as_str();
        let input = format!("#[|one{newline}two{newline}]#three{newline}");
        let (mut editor, _store) = editor(&input);
        current!(editor).1.line_ending = ending;
        editor.change_selections(Some('a'), 1).unwrap();
        assert_eq!(editor.mode(), Mode::Insert);
        assert_text(&editor, &format!("#[{newline}|]#three{newline}"));
        assert_eq!(
            editor.first_register_value('a').as_deref(),
            Some(format!("one{newline}two{newline}").as_str())
        );
        let (view, doc) = current!(editor);
        editing::commit(doc, view);
        assert!(editing::undo(doc, view));
        assert_eq!(
            doc.text(),
            format!("one{newline}two{newline}three{newline}").as_str()
        );
        assert!(!editing::undo(doc, view));
    }
}

#[tokio::test]
async fn deletion_without_yanking_preserves_saved_registers() {
    let (mut editor, _store) = editor("#[|abc]#\n");
    editor.registers.write('a', vec!["saved".into()]).unwrap();
    editor.set_mode(Mode::Select);
    editor.delete_selections(None).unwrap();
    assert_eq!(editor.mode(), Mode::Normal);
    assert_eq!(editor.first_register_value('a').as_deref(), Some("saved"));
    assert_text(&editor, "#[\n|]#");
}

#[tokio::test]
async fn contextual_register_paste_and_replacement_work_without_a_frontend() {
    let (mut editor, _store) = editor("#[|é]#\n");
    assert!(editor.paste_register('.', PastePosition::After, 1).unwrap());
    assert_text(&editor, "é#[|é]#\n");
    editor.registers.write('a', vec!["界".into()]).unwrap();
    assert!(editor.replace_from_register('a', 2).unwrap());
    assert_text(&editor, "é#[|界界]#\n");
    let (view, doc) = current!(editor);
    assert!(editing::undo(doc, view));
    assert_eq!(doc.text(), "éé\n");
    assert!(editing::undo(doc, view));
    assert_eq!(doc.text(), "é\n");
}

#[tokio::test]
async fn absent_and_empty_registers_leave_pending_typing_uncommitted() {
    let (mut editor, _store) = editor("#[|x]#\n");
    let (view, doc) = current!(editor);
    let edit = Transaction::change(doc.text(), [(0, 1, Some("a".into()))].into_iter());
    editing::try_apply(doc, &edit, view.id).unwrap();
    assert!(!editor.paste_register('z', PastePosition::After, 1).unwrap());
    assert!(!editor.paste_register('_', PastePosition::After, 1).unwrap());
    assert!(!editor.replace_from_register('z', 1).unwrap());
    assert!(!editor.replace_from_register('_', 1).unwrap());
    editor.registers.write('e', vec![String::new()]).unwrap();
    assert!(!editor.paste_register('e', PastePosition::After, 1).unwrap());
    let (view, doc) = current!(editor);
    let edit = Transaction::change(doc.text(), [(0, 1, Some("b".into()))].into_iter());
    editing::try_apply(doc, &edit, view.id).unwrap();
    assert!(editing::undo(doc, view));
    assert_eq!(doc.text(), "x\n");
    assert!(!editing::undo(doc, view));
}

struct FailingClipboard;
impl clipboard::ClipboardBackend for FailingClipboard {
    fn name(&self, _: &clipboard::ClipboardProvider) -> String {
        "unavailable".into()
    }
    fn get_contents(
        &self,
        _: &clipboard::ClipboardProvider,
        _: clipboard::ClipboardType,
    ) -> clipboard::Result<String> {
        Err(clipboard::ClipboardError::Unavailable)
    }
    fn set_contents(
        &self,
        _: &clipboard::ClipboardProvider,
        _: &str,
        _: clipboard::ClipboardType,
    ) -> clipboard::Result<()> {
        Err(clipboard::ClipboardError::Unavailable)
    }
}

#[tokio::test]
async fn rejected_paste_keeps_pending_edits_uncommitted() {
    let (mut editor, _store) = editor("#[|abc]#\n");
    let mut doc = Document::from(
        Rope::from_str("abc\n"),
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    );
    let edit = Transaction::change(doc.text(), [(0, 1, Some("x".into()))].into_iter());
    doc.apply(edit.changes()).unwrap();
    let pending = doc.changes().clone();
    let revision = doc.snapshot().revision();
    let doc = doc.with_readonly_name("adapter source".into());
    let id = editor.new_file_from_document(Action::Replace, doc);
    editor.set_mode(Mode::Insert);
    editor.registers.write('a', vec!["value".into()]).unwrap();
    assert!(editor
        .paste_register('a', PastePosition::Cursor, 1)
        .is_err());
    assert!(editor.replace_from_register('a', 1).is_err());
    assert_eq!(editor.mode(), Mode::Insert);
    let doc = editor.document(id).unwrap();
    assert_eq!(doc.text(), "xbc\n");
    assert_eq!(doc.changes().changes(), pending.changes());
    assert_eq!(doc.snapshot().revision(), revision);
}
