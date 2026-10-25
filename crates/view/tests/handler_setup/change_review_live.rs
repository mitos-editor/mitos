//! Live review invalidation through editor events, polling, and native watching.

use std::sync::Arc;
use vcs::{FileChange, FileStatus};
use view::{current_ref, editor::Action, features::change_review::ReviewComparison};

use super::{
    blame::{git, prepare},
    change_review::{
        file_event, finish_review, review_request, settle_review_events, wait_for_review,
    },
    Fixture,
};

#[tokio::test(flavor = "multi_thread")]
async fn live_reviews_follow_buffer_edits_in_background_and_keep_other_layers() -> anyhow::Result<()>
{
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let head = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::VerticalSplit,
        None,
    );
    finish_review(&mut f, head).await?;
    let buffer = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Buffer),
        Action::Replace,
        None,
    );
    finish_review(&mut f, buffer).await?;
    let staged = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Staged),
        Action::Replace,
        None,
    );
    finish_review(&mut f, staged).await?;
    let staged_source = f
        .editor
        .document(staged)
        .unwrap()
        .review()
        .unwrap()
        .source()
        .unwrap()
        .clone();
    f.editor.switch(source, Action::Replace);
    let focus = f.editor.tree.focus();
    f.replace("first edit\n");
    f.replace("latest edit\n");
    wait_for_review(&mut f, head, |doc| {
        doc.text().to_string().contains("latest edit")
    })
    .await?;
    wait_for_review(&mut f, buffer, |doc| {
        doc.text().to_string().contains("latest edit")
    })
    .await?;
    assert_eq!(current_ref!(f.editor).1.id(), source);
    assert_eq!(f.editor.tree.focus(), focus);
    assert!(Arc::ptr_eq(
        &staged_source,
        f.editor
            .document(staged)
            .unwrap()
            .review()
            .unwrap()
            .source()
            .unwrap()
    ));
    assert_eq!(std::fs::read_to_string(path)?, "committed\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_reviews_follow_disk_index_and_head_events_without_reloading_source(
) -> anyhow::Result<()> {
    use view::file_watcher::EventType;
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let unstaged = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Unstaged),
        Action::Replace,
        None,
    );
    finish_review(&mut f, unstaged).await?;
    let staged = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Staged),
        Action::VerticalSplit,
        None,
    );
    finish_review(&mut f, staged).await?;
    std::fs::write(&path, "external edit\n")?;
    file_event(&mut f, &path, EventType::Modified);
    wait_for_review(&mut f, unstaged, |doc| {
        doc.text().to_string().contains("external edit")
    })
    .await?;
    git(f.dir.path(), &["add", "document.words"]);
    let index = f.dir.path().join(".git/index");
    file_event(&mut f, &index, EventType::Modified);
    wait_for_review(&mut f, staged, |doc| {
        doc.text().to_string().contains("external edit")
    })
    .await?;
    wait_for_review(&mut f, unstaged, |doc| {
        doc.text().to_string().contains("No text changes")
    })
    .await?;
    git(f.dir.path(), &["commit", "-m", "external commit"]);
    let head = f.dir.path().join(".git/HEAD");
    file_event(&mut f, &head, EventType::Modified);
    wait_for_review(&mut f, staged, |doc| {
        doc.text().to_string().contains("No text changes")
    })
    .await?;
    assert_eq!(
        f.editor.document(source).unwrap().text().to_string(),
        "committed\n"
    );
    assert_eq!(current_ref!(f.editor).1.id(), staged);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_reviews_poll_when_native_watching_and_buffer_reload_are_disabled(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    f.configure(|config| {
        config.files.watch.watch_vcs = false;
        config.files.reload.poll.enable = true;
        config.files.reload.poll.interval = 100;
    });
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Unstaged),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    std::fs::write(&path, "polled change\n")?;
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("polled change")
    })
    .await?;
    let source = f
        .editor
        .document(id)
        .unwrap()
        .review()
        .unwrap()
        .source()
        .unwrap()
        .clone();
    settle_review_events(&mut f).await;
    assert!(Arc::ptr_eq(
        &source,
        f.editor
            .document(id)
            .unwrap()
            .review()
            .unwrap()
            .source()
            .unwrap()
    ));
    assert!(!f.editor.document(id).unwrap().is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_buffer_reviews_follow_closing_and_reopening_the_source() -> anyhow::Result<()> {
    use view::file_watcher::EventType;
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace("unsaved\n");
    let head = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, head).await?;
    let buffer = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Buffer),
        Action::VerticalSplit,
        None,
    );
    finish_review(&mut f, buffer).await?;
    assert!(f.editor.close_document(source, true).is_ok());
    wait_for_review(&mut f, head, |doc| {
        doc.text().to_string().contains("No text changes")
    })
    .await?;
    wait_for_review(&mut f, buffer, |doc| {
        doc.text().to_string().contains("No text changes")
    })
    .await?;
    std::fs::write(&path, "on disk\n")?;
    file_event(&mut f, &path, EventType::Modified);
    wait_for_review(&mut f, head, |doc| {
        doc.text().to_string().contains("on disk")
    })
    .await?;
    wait_for_review(&mut f, buffer, |doc| {
        doc.text().to_string().contains("on disk")
    })
    .await?;
    f.editor.open(&path, Action::Replace)?;
    f.replace("reopened unsaved\n");
    wait_for_review(&mut f, head, |doc| {
        doc.text().to_string().contains("reopened unsaved")
    })
    .await?;
    wait_for_review(&mut f, buffer, |doc| {
        doc.text().to_string().contains("reopened unsaved")
    })
    .await?;
    assert_eq!(std::fs::read_to_string(path)?, "on disk\n");
    // A queued invalidation must not resurrect a review closed in the meantime.
    f.replace("pending\n");
    assert!(f.editor.close_document(head, false).is_ok());
    assert!(f.editor.close_document(buffer, false).is_ok());
    settle_review_events(&mut f).await;
    assert!(f.editor.document(head).is_none());
    assert!(f.editor.document(buffer).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_reviews_recover_from_binary_replacement_and_file_deletion() -> anyhow::Result<()> {
    use view::file_watcher::EventType;
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Unstaged),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    std::fs::write(&path, b"\0binary")?;
    file_event(&mut f, &path, EventType::Modified);
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("Binary file")
    })
    .await?;
    let notice = f.editor.document(id).unwrap().version();
    file_event(&mut f, &path, EventType::Modified);
    settle_review_events(&mut f).await;
    // Unchanged failures are cached too, without replacing the derived buffer.
    assert_eq!(notice, f.editor.document(id).unwrap().version());
    std::fs::remove_file(&path)?;
    file_event(&mut f, &path, EventType::Delete);
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("committed") && doc.review().unwrap().source().is_some()
    })
    .await?;
    std::fs::write(&path, "restored\n")?;
    file_event(&mut f, &path, EventType::Create);
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("restored")
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_events_refresh_live_reviews_after_the_source_buffer_closes() -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Staged),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    assert!(f.editor.close_document(source, false).is_ok());
    f.configure(|config| {
        config.files.watch.enable = true;
        config.files.watch.watch_vcs = true;
        config.files.reload.poll.enable = false;
        config.files.reload.enable = false;
    });
    let index = stdx::path::canonicalize_existing(&f.dir.path().join(".git/index"));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !f.editor.file_watcher.is_watching(&index)
            || !f.editor.file_watcher.is_watching(&path)
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|error| {
        anyhow::anyhow!(
            "Native roots did not become ready: {error}; index={}, file={}",
            f.editor.file_watcher.is_watching(&index),
            f.editor.file_watcher.is_watching(&path)
        )
    })?;
    std::fs::write(&path, "external staging\n")?;
    git(f.dir.path(), &["add", "document.words"]);
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("external staging")
    })
    .await?;
    let working = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::VerticalSplit,
        None,
    );
    finish_review(&mut f, working).await?;
    std::fs::write(&path, "external working file\n")?;
    wait_for_review(&mut f, working, |doc| {
        doc.text().to_string().contains("external working file")
    })
    .await?;
    assert!(f.editor.document(source).is_none());
    assert_eq!(current_ref!(f.editor).1.id(), working);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_during_initial_loading_retry_without_manual_refresh() -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    let doc = f.editor.document(source).unwrap();
    let transaction = editor_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("newer source\n".into()))].into_iter(),
    );
    f.editor
        .apply_document_changes(source, doc.version(), transaction.changes())?;
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("newer source")
    })
    .await?;
    assert_eq!(current_ref!(f.editor).1.id(), id);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_git_actions_refresh_reviews_with_all_filesystem_refresh_disabled(
) -> anyhow::Result<()> {
    use view::features::git_operations::{GitAction, GitRequest};
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Staged),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    std::fs::write(&path, "stage from editor\n")?;
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path }),
    };
    let request = GitRequest::capture(
        &f.editor,
        f.dir.path().into(),
        GitAction::File(status, vcs::operations::FileOperation::Stage),
    )?;
    request
        .prepare()
        .await?
        .admit(&mut f.editor)?
        .execute()
        .await
        .finish(&mut f.editor);
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("stage from editor")
    })
    .await?;
    Ok(())
}
