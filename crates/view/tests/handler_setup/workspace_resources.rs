use super::Fixture;
use lsp_client::UriExt as _;
use lsp_client::{lsp, OffsetEncoding, Url};
use serde_json::json;
use view::{
    current_ref,
    editor::{Action, FileOperation},
};

#[tokio::test(flavor = "multi_thread")]
async fn existing_targets_require_explicit_overwrite() -> anyhow::Result<()> {
    for kind in ["create", "rename"] {
        for (options, overwrites, ignores) in [
            (json!(null), false, false),
            (json!({"overwrite":false}), false, false),
            (json!({"ignoreIfExists":true}), false, true),
            (json!({"overwrite":true}), true, false),
            (json!({"overwrite":true,"ignoreIfExists":true}), true, false),
        ] {
            let mut fixture = Fixture::new("original\n")?;
            let source = fixture.dir.path().join("source.txt");
            let target = fixture.dir.path().join("target.txt");
            let skipped = fixture.dir.path().join("after.txt");
            std::fs::write(&source, "source contents")?;
            std::fs::write(&target, "target contents")?;
            let edit: lsp::WorkspaceEdit = serde_json::from_value(json!({"documentChanges":[
                {"kind":kind,"uri":Url::from_file_path(&target).unwrap(),
                 "oldUri":Url::from_file_path(&source).unwrap(),"newUri":Url::from_file_path(&target).unwrap(),"options":options},
                {"kind":"create","uri":Url::from_file_path(&skipped).unwrap()}
            ]}))?;
            let result = fixture
                .editor
                .apply_workspace_edit(OffsetEncoding::Utf8, &edit);
            assert_eq!(
                result.is_ok(),
                overwrites || ignores,
                "{kind} {options}: {result:?}"
            );
            if let Err(error) = result {
                assert_eq!(error.failed_change_idx, 0);
            }
            assert_eq!(
                std::fs::read_to_string(&target)?,
                if overwrites {
                    if kind == "create" {
                        ""
                    } else {
                        "source contents"
                    }
                } else {
                    "target contents"
                }
            );
            assert_eq!(source.exists(), kind == "create" || !overwrites);
            assert_eq!(skipped.exists(), overwrites || ignores);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resource_overwrite_preserves_open_destination_documents() -> anyhow::Result<()> {
    for kind in ["create", "rename"] {
        let mut fixture = Fixture::new("on disk\n")?;
        let (id, target) = {
            let doc = current_ref!(fixture.editor).1;
            (doc.id(), doc.path().unwrap().to_owned())
        };
        fixture.replace_and_commit("unsaved\n");
        let source = fixture.dir.path().join("source.txt");
        std::fs::write(&source, "source contents")?;
        let edit: lsp::WorkspaceEdit = serde_json::from_value(json!({"documentChanges":[
            {"kind":kind,"uri":Url::from_file_path(&target).unwrap(),
             "oldUri":Url::from_file_path(&source).unwrap(),"newUri":Url::from_file_path(&target).unwrap(),"options":{"overwrite":true}}
        ]}))?;
        assert!(fixture
            .editor
            .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
            .is_err());
        assert_eq!(std::fs::read_to_string(&target)?, "on disk\n");
        assert_eq!(fixture.editor.document(id).unwrap().text(), "unsaved\n");
        assert!(source.exists());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_resource_operations_reject_existing_destinations() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let source = fixture.dir.path().join("source.txt");
    let target = fixture.dir.path().join("target.txt");
    std::fs::write(&source, "source contents")?;
    std::fs::write(&target, "target contents")?;
    for operation in [
        FileOperation::Create {
            path: target.clone(),
            is_dir: false,
        },
        FileOperation::Move {
            from: source.clone(),
            to: target.clone(),
        },
    ] {
        assert!(fixture.editor.request_file_operation(operation).is_err());
        assert_eq!(std::fs::read_to_string(&source)?, "source contents");
        assert_eq!(std::fs::read_to_string(&target)?, "target contents");
    }
    // Moving a new file's buffer before its first save remains supported.
    let unsaved = fixture.dir.path().join("unsaved.txt");
    let moved = fixture.dir.path().join("moved.txt");
    let id = fixture.editor.open(&unsaved, Action::Replace)?;
    fixture.editor.request_file_operation(FileOperation::Move {
        from: unsaved,
        to: moved.clone(),
    })?;
    assert_eq!(
        fixture.editor.document(id).unwrap().path(),
        Some(stdx::path::canonicalize(&moved).as_path())
    );
    assert!(!moved.exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resource_rename_rejects_missing_sources_and_delete_honors_missing_options(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let missing = Url::from_file_path(fixture.dir.path().join("missing")).unwrap();
    let target = Url::from_file_path(fixture.dir.path().join("target")).unwrap();
    for (operation, succeeds) in [
        (
            json!({"kind":"rename","oldUri":missing,"newUri":target}),
            false,
        ),
        (json!({"kind":"delete","uri":missing}), false),
        (
            json!({"kind":"delete","uri":missing,"options":{"ignoreIfNotExists":true}}),
            true,
        ),
    ] {
        let edit = serde_json::from_value(json!({"documentChanges":[operation]}))?;
        assert_eq!(
            fixture
                .editor
                .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
                .is_ok(),
            succeeds
        );
    }
    Ok(())
}
