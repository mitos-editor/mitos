use super::Fixture;
use ::diagnostics::{Diagnostic, DiagnosticProvider, Range};
use std::sync::{Arc, Mutex};
use view::{current, current_ref, editor::Action, events::DocumentLanguageDidChange};

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
