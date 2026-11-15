//! Git source identities reuse immutable buffers without acquiring file ownership.
use super::*;
use vcs::operations::{
    history::{RevisionFile, RevisionId},
    GitRepository,
};
use view::{
    editor::Action,
    features::change_review::{HistoricalReviewRequest, ReviewLineKind},
    DocumentId,
};

fn head(directory: &Path) -> anyhow::Result<RevisionId> {
    let output = git_output(directory, &["rev-parse", "HEAD"]);
    assert!(output.status.success());
    RevisionId::parse(String::from_utf8(output.stdout)?.trim())
}

fn request(
    app: &TestApplication,
    repository: &GitRepository,
    revision: &RevisionId,
    path: &str,
) -> anyhow::Result<HistoricalReviewRequest> {
    HistoricalReviewRequest::revisions(
        &app.editor,
        repository.clone(),
        None,
        Some(RevisionFile::new(revision.clone(), path.into())?),
    )
}

async fn wait_source(
    app: &mut TestApplication,
    id: DocumentId,
    contents: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !app
            .editor
            .document(id)
            .is_some_and(|doc| doc.text().to_string().contains(contents))
        {
            run_event_loop_until_idle(app).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pinned_sources_reuse_loading_and_ready_buffers_without_file_identity() -> anyhow::Result<()>
{
    let (directory, path) = committed_file()?;
    let repository = GitRepository::discover(&path)?;
    let revision = head(directory.path())?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let request = request(&app, &repository, &revision, "file.txt")?;
    let id = app
        .editor
        .open_historical_version(request.clone(), Action::Load);
    assert_eq!(
        app.editor
            .open_historical_version(request.clone(), Action::VerticalSplit),
        id
    );
    assert_eq!(app.editor.documents().count(), 2);
    wait_source(&mut app, id, "committed").await?;
    let doc = app.editor.document_mut(id).unwrap();
    assert!(doc.path().is_none() && doc.uri().is_none() && doc.url().is_none());
    assert!(doc.is_content_readonly() && doc.is_readonly() && !doc.is_modified());
    assert_eq!(
        doc.source_path(),
        Some(repository.root().join("file.txt").as_path())
    );
    assert_eq!(doc.language_servers().count(), 0);
    assert!(doc.save(Some(path.clone()), true).is_err());
    // LSP lifecycle changes must not evict Git sources from the shared cache.
    for enabled in [true, false] {
        let mut config = (*app.editor.config()).clone();
        config.lsp.enable = enabled;
        app.apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(config)));
        assert_eq!(
            app.editor
                .open_historical_version(request.clone(), Action::Load),
            id
        );
    }
    assert!(app.editor.close_document(id, true).is_ok());
    let reopened = app.editor.open_historical_version(request, Action::Load);
    assert_ne!(reopened, id);
    wait_source(&mut app, reopened, "committed").await?;
    assert_eq!(std::fs::read_to_string(path)?, "committed\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn source_identity_includes_repository_revision_and_path() -> anyhow::Result<()> {
    let (directory, path) = committed_file()?;
    let repository = GitRepository::discover(&path)?;
    let first_revision = head(directory.path())?;
    std::fs::write(directory.path().join("other.txt"), "other content\n")?;
    git(directory.path(), &["add", "other.txt"]);
    git(directory.path(), &["commit", "-m", "another file"]);
    let second_revision = head(directory.path())?;
    git(directory.path(), &["clone", "--no-hardlinks", ".", "clone"]);
    let clone = GitRepository::discover(&directory.path().join("clone/file.txt"))?;
    let mut app = AppBuilder::new().with_file(path, None).build()?;
    let mut ids = Vec::new();
    for (repository, revision, path, text) in [
        (&repository, &first_revision, "file.txt", "committed"),
        (&repository, &second_revision, "file.txt", "committed"),
        (&repository, &second_revision, "other.txt", "other content"),
        (&clone, &second_revision, "file.txt", "committed"),
    ] {
        let request = request(&app, repository, revision, path)?;
        let id = app.editor.open_historical_version(request, Action::Load);
        assert!(!ids.contains(&id));
        ids.push(id);
        wait_source(&mut app, id, text).await?;
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn captured_buffer_sources_keep_independent_snapshots() -> anyhow::Result<()> {
    let (directory, path) = committed_file()?;
    let repository = GitRepository::discover(&path)?;
    let endpoint = RevisionFile::new(head(directory.path())?, "file.txt".into())?;
    let mut app = AppBuilder::new().with_file(path.clone(), None).build()?;
    let document = current_ref!(app.editor).1.id();
    let mut snapshots = Vec::new();
    for text in ["first snapshot\n", "second snapshot\n"] {
        let (view, doc) = current!(app.editor);
        let change = editor_core::Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        view::editing::apply(doc, &change, view.id)?;
        let request = HistoricalReviewRequest::against_buffer(
            &app.editor,
            repository.clone(),
            endpoint.clone(),
            document,
        )?;
        let id = app.editor.open_historical_version(request, Action::Load);
        wait_source(&mut app, id, text).await?;
        snapshots.push(id);
    }
    assert_ne!(snapshots[0], snapshots[1]);
    for (id, text) in snapshots
        .into_iter()
        .zip(["first snapshot\n", "second snapshot\n"])
    {
        assert_eq!(app.editor.document(id).unwrap().text().to_string(), text);
    }
    assert_eq!(std::fs::read_to_string(path)?, "committed\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_loads_cannot_publish_and_failed_loads_can_be_retried() -> anyhow::Result<()> {
    let (directory, path) = committed_file()?;
    let repository = GitRepository::discover(&path)?;
    let revision = head(directory.path())?;
    let mut app = AppBuilder::new().with_file(path, None).build()?;
    let valid = request(&app, &repository, &revision, "file.txt")?;
    let closed = app
        .editor
        .open_historical_version(valid.clone(), Action::Load);
    assert!(app.editor.close_document(closed, true).is_ok());
    let reopened = app.editor.open_historical_version(valid, Action::Load);
    assert_ne!(reopened, closed);
    wait_source(&mut app, reopened, "committed").await?;
    assert!(app.editor.document(closed).is_none());
    let missing = request(&app, &repository, &revision, "missing.txt")?;
    let failed = app
        .editor
        .open_historical_version(missing.clone(), Action::Load);
    wait_source(&mut app, failed, "Cannot open historical version").await?;
    let retry = app.editor.open_historical_version(missing, Action::Load);
    assert_ne!(retry, failed);
    wait_source(&mut app, retry, "Cannot open historical version").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn renamed_diff_sides_reuse_their_exact_historical_buffers() -> anyhow::Result<()> {
    let (directory, path) = committed_file()?;
    let repository = GitRepository::discover(&path)?;
    let before = head(directory.path())?;
    let renamed = directory.path().join("renamed.txt");
    std::fs::rename(&path, &renamed)?;
    std::fs::write(&renamed, "changed content\n")?;
    git(directory.path(), &["add", "-A"]);
    git(directory.path(), &["commit", "-m", "rename and change"]);
    let after = head(directory.path())?;
    let mut app = AppBuilder::new().with_file(&renamed, None).build()?;
    let before_request = request(&app, &repository, &before, "file.txt")?;
    let before_id = app
        .editor
        .open_historical_version(before_request, Action::Load);
    let after_request = request(&app, &repository, &after, "renamed.txt")?;
    let after_id = app
        .editor
        .open_historical_version(after_request, Action::Load);
    wait_source(&mut app, before_id, "committed").await?;
    wait_source(&mut app, after_id, "changed content").await?;
    let comparison = HistoricalReviewRequest::revisions(
        &app.editor,
        repository,
        Some(RevisionFile::new(before, "file.txt".into())?),
        Some(RevisionFile::new(after, "renamed.txt".into())?),
    )?;
    let review_id = app
        .editor
        .open_historical_review(comparison, Action::Replace);
    wait_for_review_buffer(&mut app, "changed content").await?;
    for (kind, source_id, text) in [
        (ReviewLineKind::Removed, before_id, "committed\n"),
        (ReviewLineKind::Added, after_id, "changed content\n"),
    ] {
        app.editor.switch(review_id, Action::Replace);
        let (view, doc) = current!(app.editor);
        let line = doc
            .review()
            .unwrap()
            .review()
            .lines
            .iter()
            .position(|line| line.kind == kind)
            .unwrap();
        doc.set_selection(
            view.id,
            editor_core::Selection::point(doc.text().line_to_char_idx(line)),
        );
        app.editor.open_review_source(true, Action::VerticalSplit)?;
        let (_, doc) = current_ref!(app.editor);
        assert_eq!(doc.id(), source_id);
        assert_eq!(doc.text().to_string(), text);
        assert!(doc.path().is_none() && doc.uri().is_none() && doc.is_readonly());
    }
    assert_eq!(std::fs::read_to_string(renamed)?, "changed content\n");
    assert!(app.close().await.is_empty());
    Ok(())
}
