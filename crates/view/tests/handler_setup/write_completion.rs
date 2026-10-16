use super::Fixture;
use view::{current_ref, editor::Action};

#[tokio::test(flavor = "multi_thread")]
async fn write_flush_finishes_other_writes_after_failure() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("first\n")?;
    let first = current_ref!(fixture.editor).1.id();
    let good = fixture.dir.path().join("good.txt");
    fixture.editor.save(
        first,
        Some(fixture.dir.path().join("missing/failed.txt")),
        false,
    )?;
    let second = fixture.editor.new_file(Action::Replace);
    fixture.replace_and_commit("successful\n");
    let submitted = current_ref!(fixture.editor).1.text().to_string();
    fixture.editor.save(second, Some(good.clone()), false)?;
    assert!(fixture.editor.flush_writes().await.is_err());
    assert!(!fixture.editor.has_pending_writes());
    assert_eq!(std::fs::read_to_string(&good)?, submitted);
    let saved = fixture.editor.document(second).unwrap();
    assert_eq!(
        saved.path(),
        Some(stdx::path::canonicalize(&good).as_path())
    );
    assert!(!saved.is_modified());
    assert!(fixture.editor.document(first).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn blocking_write_completion_records_the_written_revision_and_path() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    fixture.replace_and_commit("submitted\n");
    let id = current_ref!(fixture.editor).1.id();
    let target = fixture.dir.path().join("renamed.txt");
    fixture.editor.save(id, Some(target.clone()), false)?;
    fixture.replace("newer\n");
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&target)?, "submitted\n");
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(
        doc.path(),
        Some(stdx::path::canonicalize(&target).as_path())
    );
    assert_eq!(doc.text().to_string(), "newer\n");
    assert!(doc.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_document_preserves_its_accepted_writes_in_submission_order() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new("original\n")?;
    let doc = current_ref!(fixture.editor).1;
    let (id, path) = (doc.id(), doc.path().unwrap().to_owned());
    for text in ["first\n", "last\n"] {
        fixture.replace_and_commit(text);
        // Queue ordering is independent of overwrite protection: each snapshot
        // intentionally overwrites the preceding snapshot of the same file.
        fixture.editor.save(id, None::<std::path::PathBuf>, true)?;
    }
    fixture.close_current()?;
    assert!(fixture.editor.document(id).is_none());
    assert!(fixture.editor.has_pending_writes());
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(path)?, "last\n");
    assert!(!fixture.editor.has_pending_writes());
    Ok(())
}
