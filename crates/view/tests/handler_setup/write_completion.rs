use super::Fixture;
use view::{current, current_ref, editor::Action};

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
    fixture.replace("successful\n");
    let (view, doc) = current!(fixture.editor);
    view::editing::commit(doc, view);
    fixture.editor.save(second, Some(good.clone()), false)?;
    assert!(fixture.editor.flush_writes().await.is_err());
    assert_eq!(fixture.editor.write_count, 0);
    assert_eq!(std::fs::read_to_string(&good)?, "successful\n");
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
    fixture.replace("submitted\n");
    let (view, doc) = current!(fixture.editor);
    view::editing::commit(doc, view);
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
