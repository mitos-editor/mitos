use super::Fixture;
use view::{current, current_ref, editor::Action, features::formatting};

fn fixture() -> anyhow::Result<Fixture> {
    Fixture::with_formatter("original\n", "printf 'formatted\\n'")
}

async fn update(fixture: &Fixture) -> anyhow::Result<formatting::FormattingUpdate> {
    let view = current_ref!(fixture.editor).0.id;
    Ok(formatting::prepare_document(&fixture.editor, view)?
        .run()
        .await)
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_commits_one_undo_revision_without_a_frontend() -> anyhow::Result<()> {
    let mut fixture = fixture()?;
    assert!(update(&fixture).await?.apply(&mut fixture.editor)?);
    let (view, doc) = current!(fixture.editor);
    assert_eq!(doc.text().to_string(), "formatted\n");
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "original\n");
    assert!(!view::editing::undo(doc, view));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_does_not_follow_a_view_to_another_document() -> anyhow::Result<()> {
    let mut fixture = fixture()?;
    let doc = current_ref!(fixture.editor).1.id();
    let update = update(&fixture).await?;
    let other = fixture.editor.new_file(Action::Replace);
    let other_text = fixture.editor.document(other).unwrap().text().to_string();
    assert!(!update.apply(&mut fixture.editor)?);
    assert_eq!(
        fixture.editor.document(doc).unwrap().text().to_string(),
        "original\n"
    );
    assert_eq!(
        fixture.editor.document(other).unwrap().text().to_string(),
        other_text
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_is_discarded_after_switching_away_and_back() -> anyhow::Result<()> {
    let mut fixture = fixture()?;
    let doc = current_ref!(fixture.editor).1.id();
    let update = update(&fixture).await?;
    fixture.editor.new_file(Action::Replace);
    fixture.editor.switch(doc, Action::Replace);
    assert!(!update.apply(&mut fixture.editor)?);
    assert_eq!(
        fixture.editor.document(doc).unwrap().text().to_string(),
        "original\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_does_not_fall_back_to_another_split() -> anyhow::Result<()> {
    let mut fixture = fixture()?;
    let (view, doc) = current_ref!(fixture.editor);
    let (origin, id) = (view.id, doc.id());
    let update = update(&fixture).await?;
    fixture.editor.switch(id, Action::VerticalSplit);
    fixture.editor.close(origin);
    assert!(!update.apply(&mut fixture.editor)?);
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "original\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_preserves_edits_made_while_the_result_is_pending() -> anyhow::Result<()> {
    let mut fixture = fixture()?;
    let update = update(&fixture).await?;
    fixture.replace("new typing\n");
    assert!(!update.apply(&mut fixture.editor)?);
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "new typing\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_rejects_renamed_documents_and_replaced_language_configuration(
) -> anyhow::Result<()> {
    for rename in [true, false] {
        let mut fixture = fixture()?;
        let update = update(&fixture).await?;
        if rename {
            let path = fixture.dir.path().join("renamed.words");
            fixture
                .editor
                .set_doc_path(current_ref!(fixture.editor).1.id(), &path);
        } else {
            fixture.editor.apply_language_config(
                language::syntax::Loader::new(
                    toml::from_str("language = []")?,
                    loader::syntax::Resources::default(),
                )?,
                fixture.editor.theme.clone(),
            )?;
        }
        assert!(!update.apply(&mut fixture.editor)?);
        assert_eq!(
            current_ref!(fixture.editor).1.text().to_string(),
            "original\n"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_results_cannot_be_applied_to_another_editor() -> anyhow::Result<()> {
    let first = fixture()?;
    let mut second = fixture()?;
    let (first_view, first_doc) = current_ref!(first.editor);
    let (second_view, second_doc) = current_ref!(second.editor);
    assert_eq!(
        (first_view.id, first_doc.id(), first_doc.version()),
        (second_view.id, second_doc.id(), second_doc.version())
    );
    let path = first_doc.path().unwrap().to_path_buf();
    let id = second_doc.id();
    second.editor.set_doc_path(id, &path);
    second
        .editor
        .syn_loader
        .store(first.editor.syn_loader.load_full());
    assert!(!update(&first).await?.apply(&mut second.editor)?);
    assert_eq!(
        current_ref!(second.editor).1.text().to_string(),
        "original\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatter_errors_are_reported_only_for_a_current_target() -> anyhow::Result<()> {
    for rebind in [false, true] {
        let mut fixture = Fixture::with_formatter("original\n", "exit 1")?;
        let update = update(&fixture).await?;
        if rebind {
            fixture.editor.new_file(Action::Replace);
        }
        assert_eq!(update.apply(&mut fixture.editor).is_err(), !rebind);
    }
    Ok(())
}
