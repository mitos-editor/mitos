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
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text().to_string(), "original\n");
    assert!(!view::editing::undo(doc, view.id));
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

fn argument_fixture(argument: &str) -> anyhow::Result<Fixture> {
    let script = "printf '%%s' \"$1\"";
    Fixture::with_languages(
        "original\n",
        &format!(
            r#"
        [[language]]
        name = "words"
        scope = "text.words"
        roots = []
        file-types = ["words"]
        formatter = {{ command = "bash", args = ["-c", {script:?}, "formatter", {argument:?}] }}
    "#
        ),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_an_unfocused_split_uses_its_document_and_selection() -> anyhow::Result<()> {
    let mut fixture = argument_fixture("%{file_path_absolute}:%{selection}:%reg{%}:%reg{.}")?;
    let (view, doc) = current!(fixture.editor);
    let (origin, document, path, display) = (
        view.id,
        doc.id(),
        doc.path().unwrap().to_string_lossy().into_owned(),
        doc.display_name().into_owned(),
    );
    doc.set_selection(origin, editor_core::Selection::single(0, 3));
    fixture.editor.new_file(Action::VerticalSplit);
    fixture.replace("other\n");
    let update = formatting::prepare_document(&fixture.editor, origin)?
        .run()
        .await;
    assert!(update.apply(&mut fixture.editor)?);
    assert_eq!(
        fixture
            .editor
            .document(document)
            .unwrap()
            .text()
            .to_string(),
        format!("{path}:ori:{display}:ori")
    );
    assert_eq!(current_ref!(fixture.editor).1.text().to_string(), "other\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_documents_can_format_without_creating_view_state() -> anyhow::Result<()> {
    let mut fixture = argument_fixture("%{file_path_absolute}")?;
    let document = current_ref!(fixture.editor).1.id();
    let path = current_ref!(fixture.editor)
        .1
        .path()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fixture.editor.new_file(Action::Replace);
    let doc = fixture.editor.document(document).unwrap();
    let views: Vec<_> = doc.view_ids().collect();
    let change = doc.format(&fixture.editor).unwrap().await?;
    fixture
        .editor
        .apply_document_changes(document, doc.version(), change.changes())?;
    let doc = fixture.editor.document(document).unwrap();
    assert_eq!(doc.text().to_string(), path);
    assert_eq!(doc.view_ids().collect::<Vec<_>>(), views);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_document_formatters_report_missing_selection_context() -> anyhow::Result<()> {
    for variable in ["%{selection}", "%{cursor_line}", "%reg{.}", "%reg{#}"] {
        let mut fixture = argument_fixture(variable)?;
        let document = current_ref!(fixture.editor).1.id();
        fixture.editor.new_file(Action::Replace);
        let result = fixture
            .editor
            .document(document)
            .unwrap()
            .format(&fixture.editor)
            .unwrap()
            .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires a document view"));
        assert_eq!(
            fixture
                .editor
                .document(document)
                .unwrap()
                .text()
                .to_string(),
            "original\n"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn formatter_and_shell_expansions_use_workspace_or_document_parent() -> anyhow::Result<()> {
    for workspace in [true, false] {
        let script = "cat formatter.conf; echo \"$1\"";
        let mut fixture = Fixture::with_languages(
            "original\n",
            &format!(
                r#"
                [[language]]
                name = "words"
                scope = "text.words"
                roots = []
                file-types = ["words"]
                formatter = {{ command = "bash", args = ["-c", {script:?}, "formatter", "%sh{{cat formatter.conf}}"] }}
                "#
            ),
        )?;
        let root = stdx::path::normalize(fixture.dir.path().canonicalize()?);
        let nested = root.join("nested");
        std::fs::create_dir(&nested)?;
        if workspace {
            std::fs::create_dir(root.join(".mitos"))?;
        }
        std::fs::write(root.join("formatter.conf"), "workspace")?;
        std::fs::write(nested.join("formatter.conf"), "parent")?;
        let path = nested.join("document.words");
        std::fs::write(&path, "original\n")?;
        fixture.editor.open(&path, Action::Replace)?;
        assert!(update(&fixture).await?.apply(&mut fixture.editor)?);
        let expected = if workspace {
            "workspaceworkspace\n"
        } else {
            "parentparent\n"
        };
        assert_eq!(current_ref!(fixture.editor).1.text().to_string(), expected);
    }
    Ok(())
}
