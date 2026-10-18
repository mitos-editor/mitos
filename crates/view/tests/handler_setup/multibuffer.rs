use std::path::PathBuf;

use editor_core::{Selection, Transaction};
use view::{
    current_ref,
    editor::Action,
    multibuffer::{MultiBufferExpand, MultiBufferLocation},
    DocumentId,
};

use super::Fixture;

fn open(f: &mut Fixture, locations: Vec<MultiBufferLocation>) -> anyhow::Result<DocumentId> {
    f.editor.open_multibuffer(locations, Action::Replace)
}

fn edit(f: &mut Fixture, id: DocumentId, start: usize, end: usize, text: &str) {
    let doc = f.editor.document_mut(id).unwrap();
    let tx = Transaction::change(doc.text(), [(start, end, Some(text.into()))].into_iter());
    doc.apply(tx.changes()).unwrap();
    doc.commit_changes();
}

fn lines() -> String {
    (0..30).map(|i| format!("line {i:02}\n")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn groups_locations_and_restores_deleted_excerpt_mappings_on_undo() -> anyhow::Result<()> {
    let mut f = Fixture::new(&lines())?;
    let source = current_ref!(f.editor).1.id();
    let id = open(
        &mut f,
        vec![
            MultiBufferLocation::Document(source, Some((3, 3))),
            MultiBufferLocation::Document(source, Some((4, 4))),
            MultiBufferLocation::Document(source, Some((22, 22))),
        ],
    )?;
    let doc = f.editor.document(id).unwrap();
    assert_eq!(doc.multibuffer().unwrap().segments().len(), 2);
    assert!(!doc.is_modified());
    let before = doc.text().clone();
    let boundary = doc.multibuffer().unwrap().segments()[0]
        .projection_range
        .end;
    edit(&mut f, id, 0, boundary, "");
    let doc = f.editor.document_mut(id).unwrap();
    assert!(doc.is_modified());
    assert_eq!(
        doc.multibuffer().unwrap().segments()[0].projection_range,
        0..0
    );
    assert!(doc.undo());
    assert_eq!(doc.text(), &before);
    assert_eq!(
        doc.multibuffer().unwrap().segments()[0].projection_range,
        0..boundary
    );
    assert!(!doc.is_modified());
    assert!(doc.redo());
    assert_eq!(
        doc.multibuffer().unwrap().segments()[0].projection_range,
        0..0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn expansion_merges_excerpts_preserves_edits_and_is_undoable() -> anyhow::Result<()> {
    let mut f = Fixture::new(&lines())?;
    let source = current_ref!(f.editor).1.id();
    let id = open(
        &mut f,
        vec![
            MultiBufferLocation::Document(source, Some((4, 4))),
            MultiBufferLocation::Document(source, Some((13, 13))),
        ],
    )?;
    let view = current_ref!(f.editor).0.id;
    let boundary = f
        .editor
        .document(id)
        .unwrap()
        .multibuffer()
        .unwrap()
        .segments()[0]
        .projection_range
        .end;
    edit(&mut f, id, 0, 4, "🦀 edited");
    let second = f
        .editor
        .document(id)
        .unwrap()
        .multibuffer()
        .unwrap()
        .segments()[1]
        .projection_range
        .start;
    edit(&mut f, id, second, second + 4, "second");
    let edited = f.editor.document(id).unwrap().text().clone();
    f.editor
        .document_mut(id)
        .unwrap()
        .set_selection(view, Selection::point(0));
    f.editor
        .expand_multibuffer_excerpt(id, view, MultiBufferExpand::Down)?;
    let doc = f.editor.document_mut(id).unwrap();
    assert_eq!(doc.multibuffer().unwrap().segments().len(), 1);
    assert!(doc.text().to_string().contains("🦀 edited"));
    assert!(doc.text().to_string().contains("second"));
    assert!(doc.is_modified());
    assert!(doc.undo());
    assert_eq!(doc.text(), &edited);
    assert_eq!(doc.multibuffer().unwrap().segments().len(), 2);
    assert_ne!(boundary, 0);
    assert!(doc.redo());
    assert_eq!(doc.multibuffer().unwrap().segments().len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn clean_expansion_does_not_modify_and_navigation_opens_source() -> anyhow::Result<()> {
    let mut f = Fixture::new(&lines())?;
    let source = current_ref!(f.editor).1.id();
    let id = open(
        &mut f,
        vec![MultiBufferLocation::Document(source, Some((10, 10)))],
    )?;
    let view = current_ref!(f.editor).0.id;
    f.editor
        .expand_multibuffer_excerpt(id, view, MultiBufferExpand::Both)?;
    assert!(!f.editor.document(id).unwrap().is_modified());
    assert!(f.editor.document_mut(id).unwrap().undo());
    assert!(!f.editor.document(id).unwrap().is_modified());
    f.editor
        .open_multibuffer_excerpt(id, view, Action::VerticalSplit)?;
    assert_eq!(current_ref!(f.editor).1.id(), source);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn save_updates_sources_and_history_only_becomes_clean_after_completion() -> anyhow::Result<()>
{
    let mut f = Fixture::new("é\nβ\n")?;
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = open(
        &mut f,
        vec![MultiBufferLocation::Document(source, Some((0, 0)))],
    )?;
    edit(&mut f, id, 0, 1, "🦀 longer");
    f.editor.save(id, None::<PathBuf>, false)?;
    assert!(f.editor.document(id).unwrap().is_modified());
    f.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&path)?, "🦀 longer\nβ\n");
    let doc = f.editor.document_mut(id).unwrap();
    assert!(!doc.is_modified());
    assert!(doc.undo());
    assert!(doc.is_modified());
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&path)?, "é\nβ\n");
    assert!(!f.editor.document(id).unwrap().is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_in_later_source_leaves_all_sources_and_pending_history_untouched(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("first\n")?;
    let first = current_ref!(f.editor).1.id();
    let path = f.dir.path().join("second.txt");
    std::fs::write(&path, "second\n")?;
    let second = f.editor.open(&path, Action::Load)?;
    let id = open(
        &mut f,
        vec![
            MultiBufferLocation::Document(first, None),
            MultiBufferLocation::Document(second, None),
        ],
    )?;
    edit(&mut f, id, 0, 5, "FIRST");
    edit(&mut f, second, 0, 6, "SECOND");
    assert!(f.editor.save(id, None::<PathBuf>, false).is_err());
    assert_eq!(
        f.editor.document(first).unwrap().text().to_string(),
        "first\n"
    );
    assert!(!f.editor.has_pending_writes());
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&path)?, "second\n");
    assert!(!f.editor.document(id).unwrap().is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn outside_edits_are_mapped_and_failed_writes_can_be_retried() -> anyhow::Result<()> {
    let mut f = Fixture::new(&lines())?;
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = open(
        &mut f,
        vec![MultiBufferLocation::Document(source, Some((20, 20)))],
    )?;
    edit(&mut f, id, 0, 4, "EDIT");
    edit(&mut f, source, 0, 0, "outside\n");
    // A source modified on disk must still pass the normal save conflict check.
    std::fs::write(&path, "external\n")?;
    f.editor.save(id, None::<PathBuf>, false)?;
    assert!(f.editor.flush_writes().await.is_err());
    assert!(f.editor.document(id).unwrap().is_modified());
    assert_eq!(std::fs::read_to_string(&path)?, "external\n");
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    let saved = std::fs::read_to_string(&path)?;
    assert!(saved.starts_with("outside\n"));
    assert!(saved.contains("EDIT 17\n"));
    assert!(!f.editor.document(id).unwrap().is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_during_a_pending_write_remain_modified_and_closed_sources_are_rejected(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("before\n")?;
    let source = current_ref!(f.editor).1.id();
    let id = open(&mut f, vec![MultiBufferLocation::Document(source, None)])?;
    edit(&mut f, id, 0, 6, "saved");
    f.editor.save(id, None::<PathBuf>, true)?;
    edit(&mut f, id, 0, 5, "newer");
    f.editor.flush_writes().await?;
    assert!(f.editor.document(id).unwrap().is_modified());
    assert!(f.editor.close_document(source, true).is_ok());
    assert!(f.editor.save(id, None::<PathBuf>, true).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn saving_multiple_excerpts_remaps_later_offsets_and_undo_after_expansion(
) -> anyhow::Result<()> {
    let original = lines();
    let mut f = Fixture::new(&original)?;
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = open(
        &mut f,
        vec![
            MultiBufferLocation::Document(source, Some((7, 7))),
            MultiBufferLocation::Document(source, Some((22, 22))),
        ],
    )?;
    let view = current_ref!(f.editor).0.id;
    let before_expansion = f.editor.document(id).unwrap().text().clone();
    f.editor
        .expand_multibuffer_excerpt(id, view, MultiBufferExpand::Up)?;
    edit(&mut f, id, 0, 4, "longer 🦀");
    let second = f
        .editor
        .document(id)
        .unwrap()
        .multibuffer()
        .unwrap()
        .segments()[1]
        .projection_range
        .start;
    edit(&mut f, id, second, second + 4, "other");
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    assert!(!f.editor.document(id).unwrap().is_modified());
    let written = std::fs::read_to_string(&path)?;
    assert!(written.contains("longer 🦀 01\n"));
    assert!(written.contains("other 19\n"));
    let doc = f.editor.document_mut(id).unwrap();
    assert!(doc.undo()); // second edit
    assert!(doc.undo()); // first edit
    assert!(doc.undo()); // context expansion
    assert_eq!(doc.text(), &before_expansion);
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    let written = std::fs::read_to_string(&path)?;
    // The edit in the removed context stays in its source; writing the smaller
    // excerpts must not replace the expanded source range with their short text.
    assert!(written.contains("longer 🦀 01\n"));
    assert!(written.contains("line 19\n"));
    assert_eq!(written.lines().count(), 30);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_is_published_for_the_latest_excerpt_snapshot() -> anyhow::Result<()> {
    let mut f = Fixture::with_languages(
        "{\"a\": 1}",
        r#"
        [[language]]
        name = "json"
        scope = "source.json"
        file-types = ["words"]
    "#,
    )?;
    let source = current_ref!(f.editor).1.id();
    let id = open(&mut f, vec![MultiBufferLocation::Document(source, None)])?;
    edit(&mut f, id, 6, 7, "12345");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while f
            .editor
            .document(id)
            .unwrap()
            .multibuffer_syntax()
            .is_none()
        {
            let callback = f.callbacks.recv().await.unwrap();
            callback(&mut f.editor);
        }
    })
    .await?;
    let doc = f.editor.document(id).unwrap();
    let syntax = doc.multibuffer_syntax().unwrap()[0]
        .as_ref()
        .expect("JSON excerpt should have a syntax tree");
    assert_eq!(
        syntax.tree().root_node().end_byte() as usize,
        doc.text().len_bytes()
    );
    let loader = f.editor.syn_loader.load();
    loader.set_scopes(vec![
        "string".into(),
        "constant.numeric.integer".into(),
        "punctuation.bracket".into(),
    ]);
    assert!(!doc
        .multibuffer_highlights(0..doc.text().len_chars(), &loader)
        .is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_all_submits_changed_sources_once_without_normalizing_projection_text(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&lines())?;
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = open(
        &mut f,
        vec![MultiBufferLocation::Document(source, Some((20, 20)))],
    )?;
    edit(&mut f, source, 0, 0, "outside\n");
    edit(&mut f, id, 0, 4, "EDIT");
    view::save::save_all(
        &mut f.editor,
        view::save::WriteAllOptions {
            force: false,
            write_scratch: true,
            auto_format: false,
            code_actions: false,
        },
        |editor, request| editor.save(request.doc_id, request.path, request.force),
    )?;
    f.editor.flush_writes().await?;
    let saved = std::fs::read_to_string(path)?;
    assert!(saved.starts_with("outside\n"));
    assert!(saved.contains("EDIT 17\n"));
    assert!(!f.editor.document(id).unwrap().is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn context_insertions_do_not_become_part_of_older_excerpt_mappings() -> anyhow::Result<()> {
    let mut f = Fixture::new(&lines())?;
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = open(
        &mut f,
        vec![MultiBufferLocation::Document(source, Some((10, 10)))],
    )?;
    let view = current_ref!(f.editor).0.id;
    let end = f.editor.document(id).unwrap().text().len_chars();
    f.editor
        .expand_multibuffer_excerpt(id, view, MultiBufferExpand::Down)?;
    edit(&mut f, id, end, end, "context insertion\n");
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    let doc = f.editor.document_mut(id).unwrap();
    assert!(doc.undo());
    assert!(doc.undo());
    // The edit was in context beyond this older excerpt's end.
    assert!(!doc.is_modified());
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    assert!(std::fs::read_to_string(path)?.contains("context insertion\n"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ambiguous_historical_source_boundaries_reject_writes_even_when_forced(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&lines())?;
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = open(
        &mut f,
        vec![MultiBufferLocation::Document(source, Some((10, 10)))],
    )?;
    let view = current_ref!(f.editor).0.id;
    f.editor
        .expand_multibuffer_excerpt(id, view, MultiBufferExpand::Down)?;
    let expanded_len = f.editor.document(id).unwrap().text().len_chars();
    edit(&mut f, id, 0, expanded_len, "🦀");
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    let saved = std::fs::read_to_string(&path)?;
    let doc = f.editor.document_mut(id).unwrap();
    assert!(doc.undo());
    assert!(doc.undo());
    assert!(f.editor.save(id, None::<PathBuf>, true).is_err());
    assert_eq!(std::fs::read_to_string(path)?, saved);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_cannot_fuse_characters_across_source_boundaries() -> anyhow::Result<()> {
    let mut f = Fixture::new("e\n")?;
    let first = current_ref!(f.editor).1.id();
    let path = f.dir.path().join("combining.txt");
    std::fs::write(&path, "\u{301}x")?;
    let second = f.editor.open(&path, Action::Load)?;
    let id = open(
        &mut f,
        vec![
            MultiBufferLocation::Document(first, None),
            MultiBufferLocation::Document(second, None),
        ],
    )?;
    let doc = f.editor.document_mut(id).unwrap();
    let before = doc.text().clone();
    let tx = Transaction::change(doc.text(), [(1, 2, None)].into_iter());
    assert_eq!(
        doc.apply(tx.changes()),
        Err(view::editing::EditError::InvalidExcerptBoundary)
    );
    assert_eq!(doc.text(), &before);
    assert!(!doc.is_modified());
    Ok(())
}
