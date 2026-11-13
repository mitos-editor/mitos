use super::Fixture;
use std::{
    fs::File,
    path::PathBuf,
    time::{Duration, SystemTime},
};
use view::{current_ref, editor::Action};

async fn next_write(fixture: &mut Fixture) -> view::document::DocumentSavedEventResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let view::editor::EditorEvent::DocumentSaved(result) =
                fixture.editor.wait_event().await
            {
                break result;
            }
        }
    })
    .await?
}

fn age_file(fixture: &mut Fixture) -> anyhow::Result<PathBuf> {
    let path = current_ref!(fixture.editor).1.path().unwrap().to_owned();
    File::options()
        .write(true)
        .open(&path)?
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))?;
    fixture
        .editor
        .document_mut(current_ref!(fixture.editor).1.id())
        .unwrap()
        .pickup_last_saved_time();
    Ok(path)
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_saves_preserve_snapshots_before_completions_are_delivered() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let path = age_file(&mut fixture)?;
    let id = current_ref!(fixture.editor).1.id();
    fixture.replace_and_commit("first é\n");
    fixture.editor.save(id, None::<PathBuf>, false)?;
    fixture.replace_and_commit("second é\n");
    fixture
        .editor
        .document_mut(id)
        .unwrap()
        .set_encoding("utf-16le")?;
    fixture.editor.save(id, None::<PathBuf>, false)?;
    fixture.replace("newer\n");

    let first = next_write(&mut fixture).await?;
    assert_eq!(first.text, "first é\n");
    assert_eq!(std::fs::read(&path)?, "first é\n".as_bytes());
    // The second write must recognize its predecessor even before the frontend
    // applies either completion to the document's saved revision.
    let second = next_write(&mut fixture).await?;
    assert_eq!(second.text, "second é\n");
    assert_eq!(
        std::fs::read(&path)?,
        "second é\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    );
    fixture.editor.complete_document_write(&first);
    fixture.editor.complete_document_write(&second);
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.text(), "newer\n");
    assert!(doc.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn external_changes_between_queued_saves_are_rejected() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let path = age_file(&mut fixture)?;
    let id = current_ref!(fixture.editor).1.id();
    for text in ["first\n", "second\n"] {
        fixture.replace_and_commit(text);
        fixture.editor.save(id, None::<PathBuf>, false)?;
    }
    let first = next_write(&mut fixture).await?;
    std::fs::write(&path, "external\n")?;
    File::options()
        .write(true)
        .open(&path)?
        .set_modified(first.save_time + Duration::from_secs(60))?;
    let error = next_write(&mut fixture).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("modified by an external process"));
    assert_eq!(std::fs::read_to_string(&path)?, "external\n");
    fixture.editor.complete_document_write(&first);
    assert!(fixture.editor.document(id).unwrap().is_modified());
    assert!(!fixture.editor.has_pending_writes());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_save_as_tracks_each_destination_independently() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let original = age_file(&mut fixture)?;
    let target = fixture.dir.path().join("other.txt");
    let id = current_ref!(fixture.editor).1.id();
    for (path, text) in [
        (&original, "first\n"),
        (&target, "other\n"),
        (&original, "last\n"),
    ] {
        fixture.replace_and_commit(text);
        fixture.editor.save(id, Some(path.clone()), false)?;
    }
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&original)?, "last\n");
    assert_eq!(std::fs::read_to_string(&target)?, "other\n");
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.path(), Some(original.as_path()));
    assert!(!doc.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn another_destination_does_not_inherit_a_previous_writes_timestamp() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let original = age_file(&mut fixture)?;
    let target = fixture.dir.path().join("external.txt");
    std::fs::write(&target, "external\n")?;
    File::options()
        .write(true)
        .open(&target)?
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(2))?;
    let id = current_ref!(fixture.editor).1.id();
    fixture.replace_and_commit("submitted\n");
    fixture.editor.save(id, None::<PathBuf>, false)?;
    fixture.editor.save(id, Some(target.clone()), false)?;

    let error = fixture.editor.flush_writes().await.unwrap_err();
    assert!(error
        .to_string()
        .contains("modified by an external process"));
    assert_eq!(std::fs::read_to_string(&original)?, "submitted\n");
    assert_eq!(std::fs::read_to_string(&target)?, "external\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_save_timestamps_do_not_override_a_new_disk_baseline() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let path = age_file(&mut fixture)?;
    let id = current_ref!(fixture.editor).1.id();
    fixture.replace_and_commit("saved\n");
    fixture.editor.save(id, None::<PathBuf>, false)?;
    fixture.editor.flush_writes().await?;
    let file = File::options().write(true).open(&path)?;
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(2))?;
    fixture
        .editor
        .document_mut(id)
        .unwrap()
        .pickup_last_saved_time();

    std::fs::write(&path, "external\n")?;
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(3))?;
    fixture.editor.save(id, None::<PathBuf>, false)?;
    let error = fixture.editor.flush_writes().await.unwrap_err();
    assert!(error
        .to_string()
        .contains("modified by an external process"));
    assert_eq!(std::fs::read_to_string(&path)?, "external\n");
    Ok(())
}

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
    age_file(&mut fixture)?;
    let doc = current_ref!(fixture.editor).1;
    let (id, path) = (doc.id(), doc.path().unwrap().to_owned());
    for text in ["first\n", "last\n"] {
        fixture.replace_and_commit(text);
        fixture.editor.save(id, None::<std::path::PathBuf>, false)?;
    }
    fixture.close_current()?;
    assert!(fixture.editor.document(id).is_none());
    assert!(fixture.editor.has_pending_writes());
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(path)?, "last\n");
    assert!(!fixture.editor.has_pending_writes());
    Ok(())
}
