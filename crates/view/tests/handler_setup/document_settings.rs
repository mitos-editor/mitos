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
