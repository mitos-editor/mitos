use super::Fixture;
use editor_core::diagnostic::{Diagnostic, DiagnosticProvider, LanguageServerId, Range, Severity};
use slotmap::Key;
use view::{current, current_ref};

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
async fn language_refresh_still_invalidates_stale_spelling_findings() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("word\n")?;
    let id = current_ref!(fixture.editor).1.id();
    current!(fixture.editor).1.replace_diagnostics(
        [
            diagnostic(DiagnosticProvider::Spelling, "spelling"),
            diagnostic(
                DiagnosticProvider::Lsp {
                    server_id: LanguageServerId::null(),
                    identifier: None,
                },
                "cached lsp",
            ),
        ],
        &[],
        None,
    );
    fixture.editor.refresh_doc_language(id);
    let diagnostics = fixture.editor.document(id).unwrap().diagnostics();
    assert!(diagnostics.is_empty());
    Ok(())
}
