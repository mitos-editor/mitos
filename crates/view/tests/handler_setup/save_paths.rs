use super::Fixture;
use lsp_client::UriExt as _;
use std::path::{Path, PathBuf};
use view::{current_ref, editor::FileOperation};

fn rename(from: &Path, to: &Path) -> FileOperation {
    FileOperation::Move {
        from: from.to_owned(),
        to: to.to_owned(),
    }
}

fn resource_edit(operation: serde_json::Value) -> lsp_client::lsp::WorkspaceEdit {
    serde_json::from_value(serde_json::json!({"documentChanges":[operation]})).unwrap()
}

fn assert_save_conflict(result: Result<(), view::workspace_edit::ApplyEditError>) {
    let error = result.unwrap_err();
    assert!(
        matches!(error.kind, view::workspace_edit::ApplyEditErrorKind::IoError(error)
        if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_saves_prevent_rename_until_the_write_completes() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let doc = current_ref!(fixture.editor).1;
    let (id, source) = (doc.id(), doc.path().unwrap().to_owned());
    let target = fixture.dir.path().join("renamed.txt");
    fixture.replace_and_commit("saved\n");
    // The queue owns this write, but no I/O has started. No sleeps or scheduling
    // assumptions are needed to put rename ahead of the accepted save.
    fixture.editor.save(id, None::<std::path::PathBuf>, true)?;
    assert!(fixture
        .editor
        .request_file_operation(FileOperation::Move {
            from: source.clone(),
            to: target.clone(),
        })
        .is_err());
    assert!(source.exists() && !target.exists());
    fixture.editor.flush_writes().await?;
    fixture.editor.request_file_operation(FileOperation::Move {
        from: source.clone(),
        to: target.clone(),
    })?;
    assert!(!source.exists());
    assert_eq!(std::fs::read_to_string(&target)?, "saved\n");
    assert_eq!(
        fixture.editor.document(id).unwrap().path(),
        Some(stdx::path::canonicalize(&target).as_path())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn save_as_reserves_both_paths_after_document_closure() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let doc = current_ref!(fixture.editor).1;
    let (id, source) = (doc.id(), doc.path().unwrap().to_owned());
    let target = fixture.dir.path().join("saved-as.txt");
    let moved = fixture.dir.path().join("moved.txt");
    fixture.replace_and_commit("saved\n");
    fixture.editor.save(id, Some(target.clone()), true)?;
    fixture.close_current()?;
    assert!(fixture
        .editor
        .request_file_operation(rename(&source, &moved))
        .is_err());
    for operation in [
        serde_json::json!({"kind":"delete","uri":lsp_client::Url::from_file_path(&source).unwrap()}),
        serde_json::json!({"kind":"create","uri":lsp_client::Url::from_file_path(&target).unwrap(),"options":{"overwrite":true}}),
        serde_json::json!({"kind":"rename","oldUri":lsp_client::Url::from_file_path(&source).unwrap(),"newUri":lsp_client::Url::from_file_path(&moved).unwrap()}),
    ] {
        assert_save_conflict(
            fixture
                .editor
                .apply_workspace_edit(lsp_client::OffsetEncoding::Utf8, &resource_edit(operation)),
        );
    }
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&target)?, "saved\n");
    fixture
        .editor
        .request_file_operation(rename(&source, &moved))?;
    assert_eq!(std::fs::read_to_string(&moved)?, "original\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_operations_wait_for_child_saves_without_blocking_other_paths(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let directory = fixture.dir.path().join("directory");
    let moved = fixture.dir.path().join("moved");
    std::fs::create_dir(&directory)?;
    let child = directory.join("child.txt");
    std::fs::write(&child, "child\n")?;
    let id = fixture.editor.open(&child, view::editor::Action::Replace)?;
    fixture.editor.save(id, None::<PathBuf>, true)?;
    assert!(fixture
        .editor
        .request_file_operation(rename(&directory, &moved))
        .is_err());
    let deletion = resource_edit(
        serde_json::json!({"kind":"delete","uri":lsp_client::Url::from_file_path(&directory).unwrap(),"options":{"recursive":true}}),
    );
    assert_save_conflict(
        fixture
            .editor
            .apply_workspace_edit(lsp_client::OffsetEncoding::Utf8, &deletion),
    );
    let unrelated = fixture.dir.path().join("unrelated.txt");
    let unrelated_moved = fixture.dir.path().join("unrelated-moved.txt");
    std::fs::write(&unrelated, "other")?;
    fixture
        .editor
        .request_file_operation(rename(&unrelated, &unrelated_moved))?;
    fixture.editor.flush_writes().await?;
    fixture
        .editor
        .request_file_operation(rename(&directory, &moved))?;
    assert_eq!(std::fs::read_to_string(moved.join("child.txt"))?, "child\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_writes_release_their_paths() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let doc = current_ref!(fixture.editor).1;
    let (id, source) = (doc.id(), doc.path().unwrap().to_owned());
    let target = fixture.dir.path().join("renamed.txt");
    fixture.editor.save(
        id,
        Some(fixture.dir.path().join("missing/output.txt")),
        false,
    )?;
    assert!(fixture
        .editor
        .request_file_operation(rename(&source, &target))
        .is_err());
    assert!(fixture.editor.flush_writes().await.is_err());
    fixture
        .editor
        .request_file_operation(rename(&source, &target))?;
    assert_eq!(std::fs::read_to_string(&target)?, "original\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn completion_delivery_keeps_paths_reserved_until_applied() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let doc = current_ref!(fixture.editor).1;
    let (id, source) = (doc.id(), doc.path().unwrap().to_owned());
    let target = fixture.dir.path().join("renamed.txt");
    fixture.editor.save(id, None::<PathBuf>, true)?;
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let view::editor::EditorEvent::DocumentSaved(result) =
                fixture.editor.wait_event().await
            {
                break result;
            }
        }
    })
    .await??;
    let clone = event.clone();
    assert!(fixture
        .editor
        .request_file_operation(rename(&source, &target))
        .is_err());
    fixture.editor.complete_document_write(&event);
    // Applying completion releases its paths even when a clone remains alive.
    fixture
        .editor
        .request_file_operation(rename(&source, &target))?;
    assert_eq!(
        fixture.editor.document(id).unwrap().path(),
        Some(stdx::path::canonicalize(&target).as_path())
    );
    drop(clone);
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn pending_writes_reserve_symlink_targets_and_aliases() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let doc = current_ref!(fixture.editor).1;
    let id = doc.id();
    let source = fixture.dir.path().join("real.txt");
    std::fs::write(&source, "real\n")?;
    let alias = fixture.dir.path().join("alias.txt");
    let moved = fixture.dir.path().join("moved.txt");
    std::os::unix::fs::symlink(&source, &alias)?;
    fixture.editor.save(id, Some(alias.clone()), true)?;
    assert!(fixture
        .editor
        .request_file_operation(rename(&source, &moved))
        .is_err());
    fixture.editor.flush_writes().await?;
    // Exercise the reverse direction: a mutation using a symlink alias must
    // conflict with a write reserved under the real source path.
    fixture.editor.set_doc_path(id, &source);
    fixture.editor.save(id, None::<PathBuf>, true)?;
    assert!(fixture
        .editor
        .request_file_operation(rename(&alias, &moved))
        .is_err());
    fixture.editor.flush_writes().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn open_destination_protection_follows_symlink_aliases() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let source = current_ref!(fixture.editor).1.path().unwrap().to_owned();
    fixture.replace_and_commit("unsaved\n");
    let alias = fixture.dir.path().join("alias.txt");
    std::os::unix::fs::symlink(&source, &alias)?;
    let edit = resource_edit(
        serde_json::json!({"kind":"create","uri":lsp_client::Url::from_file_path(&alias).unwrap(),"options":{"overwrite":true}}),
    );
    assert!(fixture
        .editor
        .apply_workspace_edit(lsp_client::OffsetEncoding::Utf8, &edit)
        .is_err());
    assert_eq!(std::fs::read_to_string(source)?, "original\n");
    assert_eq!(current_ref!(fixture.editor).1.text(), "unsaved\n");
    Ok(())
}
