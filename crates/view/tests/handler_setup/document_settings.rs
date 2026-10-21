use super::Fixture;
use ::diagnostics::{Diagnostic, DiagnosticProvider, Range};
use std::sync::{Arc, Mutex};
use view::{
    current, current_ref,
    editor::Action,
    events::{ConfigDidChange, DocumentLanguageDidChange},
};

const LANGUAGES: &str = r#"
[[language]]
name = "first"
scope = "text.first"
file-types = ["words"]
roots = []
indent = { tab-width = 2, unit = "  " }
[[language]]
name = "second"
scope = "text.second"
file-types = ["other"]
roots = []
indent = { tab-width = 4, unit = "    " }
"#;

#[tokio::test]
async fn language_changes_preserve_hidden_text_and_focus_and_reject_invalid_names(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::with_languages("word\n", LANGUAGES)?;
    let id = current_ref!(fixture.editor).1.id();
    let snapshot = fixture.editor.document(id).unwrap().snapshot();
    let focused = fixture.editor.new_file(Action::Replace);
    let notifications = Arc::new(Mutex::new(Vec::new()));
    let observed = notifications.clone();
    event::register_hook!(move |event: &mut DocumentLanguageDidChange<'_>| {
        let doc = event.editor.document(event.doc).unwrap();
        observed
            .lock()
            .unwrap()
            .push(doc.language_name().map(str::to_owned));
        assert_eq!(current_ref!(event.editor).1.id(), focused);
        Ok(())
    });
    fixture.editor.set_document_language(id, Some("second"))?;
    assert_eq!(
        fixture.editor.document(id).unwrap().language_name(),
        Some("second")
    );
    assert_eq!(fixture.editor.document(id).unwrap().tab_width(), 4);
    assert!(fixture
        .editor
        .set_document_language(id, Some("missing"))
        .is_err());
    assert_eq!(
        fixture.editor.document(id).unwrap().language_name(),
        Some("second")
    );
    fixture.editor.set_document_language(id, None)?;
    let doc = fixture.editor.document(id).unwrap();
    assert!(doc.language_name().is_none());
    assert_eq!(doc.text(), snapshot.text());
    assert_eq!(doc.version(), snapshot.version());
    assert!(!doc.is_modified());
    assert_eq!(
        *notifications.lock().unwrap(),
        [Some("second".into()), None]
    );
    Ok(())
}

#[tokio::test]
async fn dictionary_overrides_disable_and_restore_inheritance_and_invalidate_checks(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::with_config(
        "word\n",
        LANGUAGES,
        loader::syntax::Resources::default(),
        |config| {
            config.spelling.languages = Some(vec!["en_US".parse().unwrap()]);
        },
    )?;
    let id = current_ref!(fixture.editor).1.id();
    let previous = fixture.editor.open_spelling_request(id);
    let provider = DiagnosticProvider::Spelling;
    current!(fixture.editor).1.replace_diagnostics(
        [Diagnostic {
            range: Range { start: 0, end: 4 },
            starts_at_word: true,
            ends_at_word: true,
            zero_width: false,
            line: 0,
            message: "old spelling".into(),
            severity: None,
            code: None,
            provider: provider.clone(),
            tags: Vec::new(),
            source: None,
            data: None,
        }],
        &[],
        Some(&provider),
    );
    fixture
        .editor
        .set_document_spelling_languages(id, Some(vec!["en_GB".parse()?]))?;
    assert!(previous.is_canceled());
    let doc = fixture.editor.document(id).unwrap();
    assert!(doc.diagnostics().is_empty());
    assert_eq!(doc.spelling_languages()[0].as_str(), "en_GB");
    fixture
        .editor
        .set_document_spelling_languages(id, Some(Vec::new()))?;
    assert!(fixture
        .editor
        .document(id)
        .unwrap()
        .spelling_languages()
        .is_empty());
    fixture.editor.set_document_spelling_languages(id, None)?;
    assert_eq!(
        fixture.editor.document(id).unwrap().spelling_languages()[0].as_str(),
        "en_US"
    );
    Ok(())
}

#[tokio::test]
async fn line_ending_conversion_maps_split_selections_and_has_one_undo_step() -> anyhow::Result<()>
{
    use buffer::HistoryAction;
    use editor_core::{LineEnding, Selection};
    let mut f = Fixture::new("é\r\nx\nlast")?;
    let (view, doc) = current!(f.editor);
    let id = doc.id();
    let origin = view.id;
    let original = Selection::single(3, 4);
    doc.set_selection(origin, original.clone());
    view.push_jump(doc, (id, Selection::point(5)));
    f.editor.switch(id, Action::VerticalSplit);
    let other = current_ref!(f.editor).0.id;
    current!(f.editor)
        .1
        .set_selection(other, Selection::point(5));
    f.editor.focus(origin);
    f.editor.set_line_ending(LineEnding::LF)?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "é\nx\nlast");
    assert_eq!(doc.line_ending, LineEnding::LF);
    assert_eq!(doc.selection(origin), &Selection::single(2, 3));
    assert_eq!(doc.selection(other), &Selection::single(4, 5));
    assert_eq!(view.jumps.iter().next_back().unwrap().1.primary().anchor, 4);
    assert!(f.editor.change_history(HistoryAction::Undo));
    assert_eq!(current_ref!(f.editor).1.text(), "é\r\nx\nlast");
    assert_eq!(current_ref!(f.editor).1.selection(origin), &original);
    assert!(!f.editor.change_history(HistoryAction::Undo));
    assert!(f.editor.change_history(HistoryAction::Redo));
    assert_eq!(current_ref!(f.editor).1.text(), "é\nx\nlast");
    Ok(())
}

#[tokio::test]
async fn indentation_choice_for_a_hidden_document_preserves_text_and_focus() -> anyhow::Result<()> {
    let mut f = Fixture::new("a\n")?;
    let id = current_ref!(f.editor).1.id();
    let focused = f.editor.new_file(Action::Replace);
    f.editor
        .set_document_indent_style(id, editor_core::indent::IndentStyle::Spaces(2))?;
    let doc = f.editor.document(id).unwrap();
    assert_eq!(doc.indent_width(), 2);
    assert_eq!(doc.text(), "a\n");
    assert!(doc.changes().is_empty());
    assert_eq!(current_ref!(f.editor).1.id(), focused);
    Ok(())
}

#[tokio::test]
async fn editorconfig_toggle_refreshes_hidden_documents_before_config_observers(
) -> anyhow::Result<()> {
    let mut f = Fixture::with_config(
        "word\n",
        LANGUAGES,
        loader::syntax::Resources::default(),
        |config| {
            config.text_width = 88;
            config.files.save.insert_final_newline = true;
            config.files.save.trim_trailing_whitespace = true;
        },
    )?;
    std::fs::write(f.dir.path().join(".editorconfig"), "root = true\n[*]\nindent_style = space\nindent_size = 6\nend_of_line = crlf\nmax_line_length = 42\ninsert_final_newline = false\ntrim_trailing_whitespace = false\n")?;
    let id = current_ref!(f.editor).1.id();
    f.editor.refresh_doc_language(id);
    let snapshot = f.editor.document(id).unwrap().snapshot();
    let focused = f.editor.new_file(Action::Replace);
    event::register_hook!(move |event: &mut ConfigDidChange<'_>| {
        let doc = event.editor.document(id).unwrap();
        assert_eq!(
            doc.text_width(),
            if event.new.files.editor_config {
                42
            } else {
                88
            }
        );
        Ok(())
    });
    for enabled in [false, true] {
        f.configure(|config| config.files.editor_config = enabled);
        let doc = f.editor.document(id).unwrap();
        assert_eq!(doc.indent_width(), if enabled { 6 } else { 2 });
        assert_eq!(doc.tab_width(), if enabled { 6 } else { 2 });
        assert_eq!(
            doc.line_ending,
            if enabled {
                editor_core::LineEnding::Crlf
            } else {
                editor_core::LineEnding::LF
            }
        );
        assert_eq!(doc.insert_final_newline(), !enabled);
        assert_eq!(doc.trim_trailing_whitespace(), !enabled);
        assert_eq!(doc.text(), snapshot.text());
        assert_eq!(doc.version(), snapshot.version());
        assert_eq!(current_ref!(f.editor).1.id(), focused);
    }
    Ok(())
}

#[tokio::test]
async fn language_reload_resolves_current_editorconfig_and_language_settings() -> anyhow::Result<()>
{
    let mut f = Fixture::with_languages("word\n", LANGUAGES)?;
    let path = f.dir.path().join(".editorconfig");
    let id = current_ref!(f.editor).1.id();
    std::fs::write(
        &path,
        "root = true\n[*]\nindent_style = space\nindent_size = 6\nmax_line_length = 42\n",
    )?;
    f.editor.refresh_doc_language(id);
    assert_eq!(f.editor.document(id).unwrap().indent_width(), 6);
    let languages = LANGUAGES.replace(
        "tab-width = 2, unit = \"  \"",
        "tab-width = 3, unit = \"   \"",
    );
    for (contents, width, indent) in [
        (
            "root = true\n[*]\nindent_style = space\nindent_size = 4\nmax_line_length = 60\n",
            60,
            4,
        ),
        ("root = true\n", f.editor.config().text_width, 3),
    ] {
        std::fs::write(&path, contents)?;
        let loader = language::syntax::Loader::new(
            toml::from_str(&languages)?,
            loader::syntax::Resources::default(),
        )?;
        let theme = f.editor.theme.clone();
        f.editor.apply_language_config(loader, theme)?;
        let doc = f.editor.document(id).unwrap();
        assert_eq!(doc.text_width(), width);
        assert_eq!(doc.indent_width(), indent);
        assert_eq!(doc.text(), "word\n");
        assert!(!doc.is_modified());
    }
    Ok(())
}

#[tokio::test]
async fn path_changes_replace_editorconfig_and_unrelated_config_keeps_manual_indentation(
) -> anyhow::Result<()> {
    let mut f = Fixture::with_languages("word\n", LANGUAGES)?;
    std::fs::write(f.dir.path().join(".editorconfig"), "root = true\n[*.words]\nindent_style = space\nindent_size = 6\nmax_line_length = 42\n[*.other]\nmax_line_length = 72\n")?;
    let id = current_ref!(f.editor).1.id();
    f.editor.refresh_doc_language(id);
    f.editor
        .set_doc_path(id, &f.dir.path().join("renamed.other"));
    let doc = f.editor.document(id).unwrap();
    assert_eq!(doc.text_width(), 72);
    assert_eq!(doc.indent_width(), 4);
    f.editor
        .set_document_indent_style(id, editor_core::indent::IndentStyle::Spaces(7))?;
    f.configure(|config| config.text_width = 99);
    assert_eq!(f.editor.document(id).unwrap().indent_width(), 7);
    assert_eq!(f.editor.document(id).unwrap().text_width(), 72);
    assert_eq!(f.editor.document(id).unwrap().text(), "word\n");
    Ok(())
}
