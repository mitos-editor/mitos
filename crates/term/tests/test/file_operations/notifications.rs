use std::path::Path;

use lsp_client::{lsp, OffsetEncoding, Url};
use serde_json::{json, Value};
use view::{current_ref, editor::Action};

use super::fixture::Fixture;

fn uri(path: &Path) -> Url {
    Url::from_file_path(stdx::path::canonicalize(path)).unwrap()
}

fn apply(
    fixture: &mut Fixture,
    operations: Value,
) -> Result<(), view::workspace_edit::ApplyEditError> {
    let edit: lsp::WorkspaceEdit =
        serde_json::from_value(json!({"documentChanges":operations})).unwrap();
    fixture
        .app
        .editor
        .apply_workspace_edit(OffsetEncoding::Utf8, &edit)
}

#[tokio::test(flavor = "multi_thread")]
async fn only_applied_resource_changes_notify_language_servers() -> anyhow::Result<()> {
    let mut fixture = Fixture::new().await?;
    let (id, version, source) = {
        let doc = current_ref!(fixture.app.editor).1;
        (doc.id(), doc.version(), uri(doc.path().unwrap()))
    };
    let existing = fixture.dir.path().join("existing.scope");
    std::fs::write(&existing, "existing\n")?;
    let existing = uri(&existing);
    let missing = uri(&fixture.dir.path().join("missing.scope"));
    apply(
        &mut fixture,
        json!([
            {"kind":"create","uri":source,"options":{"ignoreIfExists":true}},
            {"kind":"rename","oldUri":source,"newUri":existing,"options":{"ignoreIfExists":true}},
            {"kind":"rename","oldUri":source,"newUri":source},
            {"kind":"delete","uri":missing,"options":{"ignoreIfNotExists":true}}
        ]),
    )?;
    for operation in [
        json!({"kind":"create","uri":existing}),
        json!({"kind":"rename","oldUri":source,"newUri":existing}),
        json!({"kind":"delete","uri":missing}),
    ] {
        assert!(apply(&mut fixture, json!([operation])).is_err());
    }
    let doc = fixture.app.editor.document(id).unwrap();
    assert_eq!(uri(doc.path().unwrap()), source);
    assert_eq!(doc.version(), version);
    assert_eq!(doc.text(), "source\n");
    assert_eq!(
        std::fs::read_to_string(existing.to_file_path().unwrap())?,
        "existing\n"
    );

    let created = uri(&fixture.dir.path().join("created.scope"));
    let renamed = uri(&fixture.dir.path().join("renamed.scope"));
    apply(
        &mut fixture,
        json!([
            {"kind":"create","uri":created},
            {"kind":"rename","oldUri":created,"newUri":renamed},
            {"kind":"delete","uri":renamed}
        ]),
    )?;
    let log = fixture.flush_notifications().await?;
    let notifications: Vec<_> = log
        .iter()
        .filter(|message| {
            message["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("workspace/did"))
        })
        .collect();
    assert_eq!(notifications.len(), 3);
    assert_eq!(notifications[0]["method"], "workspace/didCreateFiles");
    assert_eq!(
        notifications[0]["params"]["files"][0]["uri"],
        created.as_str()
    );
    assert_eq!(notifications[1]["method"], "workspace/didRenameFiles");
    assert_eq!(
        notifications[1]["params"]["files"][0]["oldUri"],
        created.as_str()
    );
    assert_eq!(
        notifications[1]["params"]["files"][0]["newUri"],
        renamed.as_str()
    );
    assert_eq!(notifications[2]["method"], "workspace/didDeleteFiles");
    assert_eq!(
        notifications[2]["params"]["files"][0]["uri"],
        renamed.as_str()
    );
    assert!(!log
        .iter()
        .any(|message| message["method"] == "textDocument/didClose"));
    assert_eq!(
        log.iter()
            .filter(|message| message["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_rename_reopens_child_documents_before_notifying_the_move() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new().await?;
    let directory = fixture.dir.path().join("directory");
    let destination = fixture.dir.path().join("destination");
    std::fs::create_dir_all(directory.join("nested"))?;
    let mut documents = Vec::new();
    for name in ["first.scope", "nested/second.scope"] {
        let path = directory.join(name);
        std::fs::write(&path, "on disk\n")?;
        let id = fixture.app.editor.open(&path, Action::Load)?;
        fixture.change(id, "edit")?;
        documents.push((id, fixture.app.editor.document(id).unwrap().version(), name));
    }
    apply(
        &mut fixture,
        json!([
            {"kind":"rename","oldUri":uri(&directory),"newUri":uri(&destination)}
        ]),
    )?;
    for &(id, version, name) in &documents {
        let doc = fixture.app.editor.document(id).unwrap();
        assert_eq!(
            doc.path(),
            Some(stdx::path::canonicalize(destination.join(name)).as_path())
        );
        assert_eq!(doc.version(), version);
        assert_eq!(doc.text(), "typed on disk\n");
        assert_eq!(
            std::fs::read_to_string(destination.join(name))?,
            "on disk\n"
        );
    }
    assert!(!directory.exists());
    let log = fixture.flush_notifications().await?;
    let rename = log
        .iter()
        .position(|message| message["method"] == "workspace/didRenameFiles")
        .unwrap();
    assert_eq!(
        log.iter()
            .filter(|message| message["method"] == "workspace/didRenameFiles")
            .count(),
        1
    );
    for (_, _, name) in documents {
        let old_uri = uri(&directory.join(name));
        let new_uri = uri(&destination.join(name));
        let closed = log
            .iter()
            .position(|message| {
                message["method"] == "textDocument/didClose"
                    && message["params"]["textDocument"]["uri"] == old_uri.as_str()
            })
            .unwrap();
        let opened = log
            .iter()
            .position(|message| {
                message["method"] == "textDocument/didOpen"
                    && message["params"]["textDocument"]["uri"] == new_uri.as_str()
            })
            .unwrap();
        assert!(closed < opened && opened < rename);
        assert_eq!(
            log[opened]["params"]["textDocument"]["text"],
            "typed on disk\n"
        );
    }
    Ok(())
}
