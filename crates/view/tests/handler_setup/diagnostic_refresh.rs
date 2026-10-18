use super::Fixture;
use ::diagnostics::{Diagnostic, DiagnosticProvider, LanguageServerId, Range, Severity};
use slotmap::Key;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use view::{current, events::DiagnosticsDidChange};

fn diagnostic(provider: DiagnosticProvider, message: &str) -> Diagnostic {
    Diagnostic {
        range: Range { start: 0, end: 1 },
        ends_at_word: false,
        starts_at_word: false,
        zero_width: false,
        line: 0,
        message: message.into(),
        severity: Some(Severity::Warning),
        code: None,
        provider,
        tags: Vec::new(),
        source: None,
        data: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_lsp_diagnostics_preserves_spelling_and_replaces_all_lsp_namespaces(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("word\n")?;
    let server = LanguageServerId::null();
    let push = DiagnosticProvider::Lsp {
        server_id: server,
        identifier: None,
    };
    let pull = DiagnosticProvider::Lsp {
        server_id: server,
        identifier: Some("pull".into()),
    };
    let doc = current!(fixture.editor).1;
    doc.replace_diagnostics(
        [
            diagnostic(DiagnosticProvider::Spelling, "spelling"),
            diagnostic(push.clone(), "old push"),
            diagnostic(pull, "old pull"),
        ],
        &[],
        None,
    );
    doc.replace_language_server_diagnostics([diagnostic(push, "new lsp")]);
    let messages: Vec<_> = doc
        .diagnostics()
        .iter()
        .map(|d| d.message.as_ref())
        .collect();
    assert_eq!(messages.len(), 2);
    assert!(messages.contains(&"spelling"));
    assert!(messages.contains(&"new lsp"));
    doc.replace_language_server_diagnostics([]);
    assert_eq!(doc.diagnostics().len(), 1);
    assert_eq!(&*doc.diagnostics()[0].message, "spelling");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_map_points_and_ranges_and_remove_deleted_findings() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("one\n😀 two\n")?;
    let doc = current!(fixture.editor).1;
    let mut point = diagnostic(DiagnosticProvider::Spelling, "point");
    point.range = Range { start: 4, end: 4 };
    point.zero_width = true;
    let mut word = diagnostic(DiagnosticProvider::Spelling, "word");
    word.range = Range { start: 6, end: 9 };
    word.starts_at_word = true;
    word.ends_at_word = true;
    let mut deleted = diagnostic(DiagnosticProvider::Spelling, "deleted");
    deleted.range = Range { start: 0, end: 3 };
    doc.replace_diagnostics([word, deleted, point], &[], None);
    let change = editor_core::Transaction::change(doc.text(), [(0, 4, None)].into_iter());
    doc.apply(change.changes())?;
    let findings = doc.diagnostics();
    assert_eq!(findings.len(), 2);
    assert_eq!(&*findings[0].message, "point");
    assert_eq!(findings[0].range, Range { start: 0, end: 0 });
    assert_eq!(&*findings[1].message, "word");
    assert_eq!(findings[1].range, Range { start: 2, end: 5 });
    assert!(findings.iter().all(|finding| finding.line == 0));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_refresh_keeps_unchanged_sources_and_other_namespaces() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("word\n")?;
    let push = DiagnosticProvider::Lsp {
        server_id: LanguageServerId::null(),
        identifier: None,
    };
    let pull = DiagnosticProvider::Lsp {
        server_id: LanguageServerId::null(),
        identifier: Some("pull".into()),
    };
    let mut persistent = diagnostic(push.clone(), "persistent");
    persistent.source = Some("build".into());
    let doc = current!(fixture.editor).1;
    doc.replace_diagnostics(
        [
            persistent,
            diagnostic(push.clone(), "old"),
            diagnostic(pull, "pull"),
        ],
        &[],
        None,
    );
    doc.replace_diagnostics(
        [diagnostic(push.clone(), "new")],
        &["build".into()],
        Some(&push),
    );
    let messages: Vec<_> = doc
        .diagnostics()
        .iter()
        .map(|d| d.message.as_ref())
        .collect();
    assert_eq!(messages, ["persistent", "new", "pull"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_refresh_notifies_observers_after_preserving_other_providers() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new("word\n")?;
    let doc = current!(fixture.editor).1;
    let id = doc.id();
    doc.replace_diagnostics(
        [
            diagnostic(DiagnosticProvider::Spelling, "spelling"),
            diagnostic(
                DiagnosticProvider::Lsp {
                    server_id: LanguageServerId::null(),
                    identifier: None,
                },
                "stale server",
            ),
        ],
        &[],
        None,
    );
    let notifications = Arc::new(AtomicUsize::new(0));
    let observed = notifications.clone();
    event::register_hook!(move |event: &mut DiagnosticsDidChange<'_>| {
        assert_eq!(event.doc, id);
        let diagnostics = event.editor.document(id).unwrap().diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].provider, DiagnosticProvider::Spelling);
        observed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    fixture.editor.refresh_language_server_diagnostics(id);
    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    Ok(())
}
