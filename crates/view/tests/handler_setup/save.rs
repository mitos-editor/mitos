use super::Fixture;
use view::{
    current, current_ref,
    editor::Action,
    save::{self, SaveProgress, SaveTask, WriteOptions},
};

fn fixture(command: &str) -> anyhow::Result<Fixture> {
    Fixture::with_formatter("original\n", command)
}

fn formatting_save(fixture: &mut Fixture) -> anyhow::Result<SaveTask> {
    let (view, doc) = current_ref!(fixture.editor);
    let (doc, view) = (doc.id(), view.id);
    let request = save::prepare(
        &mut fixture.editor,
        doc,
        Some(view),
        None,
        WriteOptions {
            force: false,
            auto_format: true,
            code_actions: true,
        },
    )?;
    let SaveProgress::Pending(task) = save::submit(&mut fixture.editor, request)? else {
        anyhow::bail!("expected formatting stage")
    };
    Ok(task)
}

#[tokio::test(flavor = "multi_thread")]
async fn formatted_saves_follow_the_document_when_its_origin_view_switches_or_closes(
) -> anyhow::Result<()> {
    for close in [false, true] {
        let mut fixture = fixture("printf 'formatted\\n'")?;
        let (origin, doc) = current_ref!(fixture.editor);
        let (origin, id) = (origin.id, doc.id());
        let path = fixture
            .editor
            .document(id)
            .unwrap()
            .path()
            .unwrap()
            .to_owned();
        let update = formatting_save(&mut fixture)?.run().await?;
        let other = fixture.editor.new_file(if close {
            Action::VerticalSplit
        } else {
            Action::Replace
        });
        if close {
            fixture.editor.close(origin);
        }
        let unrelated = fixture.editor.document(other).unwrap().text().clone();
        let selection = current_ref!(fixture.editor)
            .1
            .selection(fixture.editor.tree.focus())
            .clone();

        assert!(matches!(
            update.apply(&mut fixture.editor)?,
            SaveProgress::Queued
        ));
        fixture.editor.flush_writes().await?;
        assert_eq!(std::fs::read_to_string(path)?, "formatted\n");
        assert_eq!(fixture.editor.document(other).unwrap().text(), &unrelated);
        assert_eq!(
            current_ref!(fixture.editor)
                .1
                .selection(fixture.editor.tree.focus()),
            &selection
        );
        // A hidden formatted document gains no synthetic selection in the unrelated split.
        if close {
            assert!(fixture
                .editor
                .document(id)
                .unwrap()
                .selections()
                .next()
                .is_none());
        }
        fixture.editor.switch(id, Action::Replace);
        let (view, doc) = current!(fixture.editor);
        assert!(view::editing::undo(doc, view.id));
        assert_eq!(doc.text().to_string(), "original\n");
        assert!(view::editing::redo(doc, view.id));
        assert_eq!(doc.text().to_string(), "formatted\n");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_and_failed_formatting_preserve_the_accepted_write() -> anyhow::Result<()> {
    for fail in [false, true] {
        let mut fixture = fixture(if fail {
            "exit 1"
        } else {
            "printf 'formatted\\n'"
        })?;
        let path = current_ref!(fixture.editor).1.path().unwrap().to_owned();
        let update = formatting_save(&mut fixture)?.run().await?;
        if !fail {
            fixture.replace("newer\n");
        }
        assert!(matches!(
            update.apply(&mut fixture.editor)?,
            SaveProgress::Queued
        ));
        fixture.editor.flush_writes().await?;
        assert_eq!(
            std::fs::read_to_string(path)?,
            if fail { "original\n" } else { "newer\n" }
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_document_stops_its_pending_save() -> anyhow::Result<()> {
    let mut fixture = fixture("printf 'formatted\\n'")?;
    let path = current_ref!(fixture.editor).1.path().unwrap().to_owned();
    let update = formatting_save(&mut fixture)?.run().await?;
    fixture.close_current()?;
    assert!(matches!(
        update.apply(&mut fixture.editor)?,
        SaveProgress::DocumentClosed
    ));
    assert!(!fixture.editor.has_pending_writes());
    assert_eq!(std::fs::read_to_string(path)?, "original\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_a_documents_path_discards_queued_formatting_but_preserves_the_write(
) -> anyhow::Result<()> {
    let mut fixture = fixture("printf 'formatted\\n'")?;
    let doc = current_ref!(fixture.editor).1;
    let (id, version, original) = (doc.id(), doc.version(), doc.path().unwrap().to_owned());
    let update = formatting_save(&mut fixture)?.run().await?;
    let renamed = fixture.dir.path().join("renamed.words");
    fixture.editor.set_doc_path(id, &renamed);
    assert_eq!(fixture.editor.document(id).unwrap().version(), version);
    assert!(matches!(
        update.apply(&mut fixture.editor)?,
        SaveProgress::Queued
    ));
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(renamed)?, "original\n");
    assert_eq!(std::fs::read_to_string(original)?, "original\n");
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "original\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_writes_keep_their_text_and_revision_when_editing_continues() -> anyhow::Result<()>
{
    use editor_core::Transaction;
    let mut fixture = Fixture::new("original\n")?;
    let path = fixture.dir.path().join("snapshot.words");
    let (_, doc) = current!(fixture.editor);
    let snapshot = doc.snapshot();
    let write = doc.save(Some(path.clone()), true)?;
    let edit = Transaction::change(doc.text(), [(0, 8, Some("newer".into()))].into_iter());
    doc.apply(edit.changes())?;
    doc.commit_changes();
    let completed = write.await?;
    assert_eq!(std::fs::read_to_string(path)?, "original\n");
    assert_eq!(&completed.text, snapshot.text());
    assert_eq!(completed.revision, snapshot.revision());
    doc.set_last_saved_revision(completed.revision, completed.save_time);
    assert_eq!(doc.text(), "newer\n");
    assert!(doc.is_modified());
    Ok(())
}
