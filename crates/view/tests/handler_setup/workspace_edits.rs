use super::Fixture;
use editor_core::Transaction;
use lsp_client::{lsp, OffsetEncoding, Url};
use view::{current, current_ref, editing::EditError, workspace_edit::ApplyEditErrorKind};

fn text_edit(uri: Url, version: Option<i32>, new_text: &str) -> lsp::TextDocumentEdit {
    lsp::TextDocumentEdit {
        text_document: lsp::OptionalVersionedTextDocumentIdentifier { uri, version },
        edits: vec![lsp::OneOf::Left(lsp::TextEdit {
            range: lsp::Range::default(),
            new_text: new_text.into(),
        })],
    }
}

fn create_file(uri: Url) -> lsp::DocumentChangeOperation {
    lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(lsp::CreateFile {
        uri,
        options: None,
        annotation_id: None,
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_workspace_edits_preserve_the_rejection_and_pending_undo_group() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new("abc\n")?;
    let (view, doc) = current!(fixture.editor);
    let version = doc.version();
    let url = doc.url().unwrap();
    let typing = Transaction::change(doc.text(), [(0, 0, Some("first".into()))].into_iter());
    view::editing::apply(doc, &typing, view.id).unwrap();
    let actual = doc.version();
    let edit = lsp::WorkspaceEdit {
        document_changes: Some(lsp::DocumentChanges::Edits(vec![text_edit(
            url,
            Some(version),
            "rejected",
        )])),
        ..Default::default()
    };
    let error = fixture
        .editor
        .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
        .unwrap_err();
    assert_eq!(error.failed_change_idx, 0);
    assert!(matches!(
        error.kind,
        ApplyEditErrorKind::Document(EditError::DocumentChanged { expected, actual: found })
            if expected == version && found == actual
    ));
    let (view, doc) = current!(fixture.editor);
    let typing = Transaction::change(doc.text(), [(0, 0, Some("second".into()))].into_iter());
    view::editing::apply(doc, &typing, view.id).unwrap();
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text(), "abc\n");
    assert!(!view::editing::undo(doc, view.id));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_edits_stop_at_binary_rejection_and_keep_completed_changes() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new("abc\n")?;
    let document_url = current_ref!(fixture.editor).1.url().unwrap();
    let created = fixture.dir.path().join("created.words");
    let skipped = fixture.dir.path().join("skipped.words");
    let binary = fixture.dir.path().join("binary.png");
    let bytes = b"\x89PNG\r\n\x1a\n\0";
    std::fs::write(&binary, bytes)?;
    let edit = lsp::WorkspaceEdit {
        document_changes: Some(lsp::DocumentChanges::Operations(vec![
            create_file(Url::from_file_path(&created).unwrap()),
            lsp::DocumentChangeOperation::Edit(text_edit(document_url, None, "prefix")),
            lsp::DocumentChangeOperation::Edit(text_edit(
                Url::from_file_path(&binary).unwrap(),
                None,
                "rejected",
            )),
            create_file(Url::from_file_path(&skipped).unwrap()),
        ])),
        ..Default::default()
    };
    let error = fixture
        .editor
        .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
        .unwrap_err();
    assert_eq!(error.failed_change_idx, 2);
    assert!(matches!(
        error.kind,
        ApplyEditErrorKind::Document(EditError::BinaryDocument)
    ));
    assert_eq!(error.kind.to_string(), "Cannot edit binary document text");
    assert!(created.exists());
    assert!(!skipped.exists());
    assert_eq!(current_ref!(fixture.editor).1.text(), "prefixabc\n");
    assert_eq!(std::fs::read(binary)?, bytes);
    let (view, doc) = current!(fixture.editor);
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text(), "abc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_edits_report_document_open_errors_with_the_path() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abc\n")?;
    let path = fixture.dir.path().join("directory");
    std::fs::create_dir(&path)?;
    let edit = lsp::WorkspaceEdit {
        document_changes: Some(lsp::DocumentChanges::Edits(vec![text_edit(
            Url::from_file_path(&path).unwrap(),
            None,
            "rejected",
        )])),
        ..Default::default()
    };
    let error = fixture
        .editor
        .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
        .unwrap_err();
    assert_eq!(error.failed_change_idx, 0);
    let ApplyEditErrorKind::OpenDocument {
        path: failed,
        source,
    } = error.kind
    else {
        panic!("expected a document open failure");
    };
    assert_eq!(failed, stdx::path::canonicalize(&path));
    assert!(matches!(
        source,
        view::document::DocumentOpenError::IrregularFile
    ));
    assert_eq!(current_ref!(fixture.editor).1.text(), "abc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_document_edits_fail_without_applying_a_subset() -> anyhow::Result<()> {
    for edits in [
        serde_json::json!([
            {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"first"},
            {"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":4}},"newText":"overlapping"}
        ]),
        serde_json::json!([
            {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"first"},
            {"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":2}},"newText":"reversed"}
        ]),
    ] {
        for versioned in [false, true] {
            let mut fixture = Fixture::new("abcdef\n")?;
            let doc = current_ref!(fixture.editor).1;
            let uri = doc.url().unwrap();
            let version = doc.version();
            let edit: lsp::WorkspaceEdit = serde_json::from_value(if versioned {
                serde_json::json!({"documentChanges":[{"textDocument":{"uri":uri,"version":version},"edits":edits}]})
            } else {
                serde_json::json!({"changes":{uri.as_str():edits}})
            })?;
            let error = fixture
                .editor
                .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
                .unwrap_err();
            assert_eq!(error.failed_change_idx, 0);
            assert!(matches!(error.kind, ApplyEditErrorKind::InvalidTextEdit(_)));
            let (view, doc) = current!(fixture.editor);
            assert_eq!(doc.text(), "abcdef\n");
            assert_eq!(doc.version(), version);
            assert!(!view::editing::undo(doc, view.id));
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_document_edits_preserve_completed_workspace_changes() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let uri = current_ref!(fixture.editor).1.url().unwrap();
    let created = fixture.dir.path().join("created.txt");
    let skipped = fixture.dir.path().join("skipped.txt");
    let bad_edit: lsp::TextDocumentEdit = serde_json::from_value(serde_json::json!({
        "textDocument":{"uri":uri,"version":null},
        "edits":[{"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":2}},"newText":"bad"}]
    }))?;
    let edit = lsp::WorkspaceEdit {
        document_changes: Some(lsp::DocumentChanges::Operations(vec![
            create_file(Url::from_file_path(&created).unwrap()),
            lsp::DocumentChangeOperation::Edit(bad_edit),
            create_file(Url::from_file_path(&skipped).unwrap()),
        ])),
        ..Default::default()
    };
    let error = fixture
        .editor
        .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
        .unwrap_err();
    assert_eq!(error.failed_change_idx, 1);
    assert!(created.exists());
    assert!(!skipped.exists());
    assert_eq!(current_ref!(fixture.editor).1.text(), "original\n");
    Ok(())
}
