//! Annotated edits require valid references and explicit consent before any mutation.

use super::Fixture;
use lsp_client::{lsp, OffsetEncoding, UriExt as _};
use serde_json::json;
use view::{current_ref, workspace_edit::ApplyEditErrorKind};

#[tokio::test]
async fn annotations_reject_before_earlier_text_or_resource_changes() -> anyhow::Result<()> {
    for resource in [false, true] {
        for unknown in [false, true] {
            let mut f = Fixture::new("original\n")?;
            let uri = current_ref!(f.editor).1.url().unwrap();
            let created = f.dir.path().join("must-not-exist");
            let first = if resource {
                json!({"kind":"create", "uri": lsp::Uri::from_file_path(&created).unwrap()})
            } else {
                json!({"textDocument":{"uri": uri}, "edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}, "newText":"changed"}]})
            };
            let edit: lsp::WorkspaceEdit = serde_json::from_value(json!({
                "documentChanges":[first, {"textDocument":{"uri":uri}, "edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"later", "annotationId":if unknown {"missing"} else {"consent"}}]}],
                "changeAnnotations":{"consent":{"label":"Review changes", "needsConfirmation":true}}
            }))?;
            let error = f
                .editor
                .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
                .unwrap_err();
            assert_eq!(error.failed_change_idx, 1);
            assert!(if unknown {
                matches!(error.kind, ApplyEditErrorKind::UnknownAnnotation(_))
            } else {
                matches!(error.kind, ApplyEditErrorKind::ConfirmationRequired)
            });
            assert_eq!(current_ref!(f.editor).1.text(), "original\n");
            assert!(!created.exists());
        }
    }
    Ok(())
}

#[tokio::test]
async fn annotation_validation_covers_create_rename_delete_and_snippets() -> anyhow::Result<()> {
    for kind in ["create", "rename", "delete", "snippet"] {
        let mut f = Fixture::new("original\n")?;
        let path = f.dir.path().join("existing");
        std::fs::write(&path, "keep")?;
        let uri = lsp::Uri::from_file_path(&path).unwrap();
        let destination = f.dir.path().join("destination");
        let op = match kind {
            "rename" => {
                json!({"kind":"rename","oldUri":uri,"newUri":lsp::Uri::from_file_path(&destination).unwrap(),"annotationId":"missing"})
            }
            "snippet" => {
                json!({"textDocument":{"uri":current_ref!(f.editor).1.url().unwrap()}, "edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"snippet":{"kind":"snippet","value":"${1:value}"},"annotationId":"missing"}]})
            }
            _ => json!({"kind":kind,"uri":uri,"annotationId":"missing"}),
        };
        let edit: lsp::WorkspaceEdit = serde_json::from_value(json!({"documentChanges":[op]}))?;
        assert!(
            matches!(
                f.editor
                    .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
                    .unwrap_err()
                    .kind,
                ApplyEditErrorKind::UnknownAnnotation(_)
            ),
            "{kind}"
        );
        assert_eq!(std::fs::read_to_string(path)?, "keep");
        assert!(!destination.exists());
        assert_eq!(current_ref!(f.editor).1.text(), "original\n");
    }
    Ok(())
}

#[tokio::test]
async fn unused_confirmation_annotations_do_not_block_plain_edits() -> anyhow::Result<()> {
    let mut f = Fixture::new("original\n")?;
    let edit: lsp::WorkspaceEdit = serde_json::from_value(json!({
        "changes":{current_ref!(f.editor).1.url().unwrap().as_str():[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}, "newText":"prefix "}]},
        "changeAnnotations":{"unused":{"label":"Not referenced", "needsConfirmation":true}}
    }))?;
    f.editor.apply_workspace_edit(OffsetEncoding::Utf8, &edit)?;
    assert_eq!(current_ref!(f.editor).1.text(), "prefix original\n");
    Ok(())
}
