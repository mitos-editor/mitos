//! Commit commands preserve ordinary editing and run mutations to completion.

use crate::{
    compositor,
    job::{Callback, Job},
    ui::PromptEvent,
};
use anyhow::{bail, ensure, Result};
use std::path::Path;
use vcs::operations::GitRepository;
use view::{
    current_ref,
    features::git_operations::{GitAction, GitRequest},
};

pub(super) fn open(cx: &mut compositor::Context, event: PromptEvent) -> Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let (view, doc) = current_ref!(cx.editor);
    let (path, workspace) = if let Some(commit) = doc.commit_message() {
        (
            commit.repository().root().to_owned(),
            commit.workspace().to_owned(),
        )
    } else if let Some(review) = doc.project_review() {
        (review.root.clone(), review.workspace.clone())
    } else if let Some(review) = doc.review() {
        (review.path().to_owned(), review.workspace().to_owned())
    } else {
        (
            doc.display_metadata()
                .source_path
                .map_or_else(stdx::env::current_working_dir, Path::to_owned),
            doc.workspace_root().to_owned(),
        )
    };
    let origin_view = view.id;
    let origin_document = doc.id();
    let origin_path = doc.path().map(Path::to_owned);
    ensure!(
        cx.editor
            .workspace_trust
            .query(&workspace, workspace::trust::TrustQuery::Git)
            .is_trusted(),
        "Git commits require workspace trust; run :workspace-trust first"
    );
    cx.jobs.callback(async move {
        let result = tokio::task::spawn_blocking(move || {
            GitRepository::discover(&path)?.commit_snapshot(true)
        })
        .await?;
        Ok(Callback::Editor(Box::new(move |editor| {
            if editor.tree.focus() != origin_view
                || !editor.tree.contains(origin_view)
                || editor.tree.get(origin_view).document_id() != origin_document
                || !editor
                    .document(origin_document)
                    .is_some_and(|doc| doc.path() == origin_path.as_deref())
            {
                editor.set_status("Commit opening canceled because its source changed");
                return;
            }
            if let Err(error) =
                result.and_then(|snapshot| editor.open_commit_message(snapshot, workspace))
            {
                editor.set_error(|| error.to_string());
            }
        })))
    });
    Ok(())
}

pub(super) fn submit(cx: &mut compositor::Context, event: PromptEvent) -> Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let (_, doc) = current_ref!(cx.editor);
    let commit = doc
        .commit_message()
        .ok_or_else(|| anyhow::anyhow!("Open a commit message with :vcs-commit first"))?;
    ensure!(
        !commit.submitted(),
        "This message was already submitted; run :vcs-commit to start another commit"
    );
    let review = cx
        .editor
        .document(commit.review())
        .and_then(|doc| doc.project_review())
        .ok_or_else(|| {
            anyhow::anyhow!("Reopen the staged review with :vcs-commit before submitting")
        })?;
    ensure!(
        !review.is_loading(),
        "Wait for the staged review to finish loading before submitting"
    );
    ensure!(
        review.comparison == view::features::change_review::ReviewComparison::Staged
            && review.root == commit.repository().root(),
        "The staged review changed; reopen it with :vcs-commit before submitting"
    );
    if let Some(section) = review
        .sections
        .iter()
        .find(|section| section.snapshot.is_none())
    {
        bail!("Staged file '{}' could not be reviewed; inspect its notice in the project review and use Git on the command line if needed", section.status.path().display());
    }
    ensure!(review.is_complete(), "The staged review is incomplete or shortened; reduce the staging area or review and commit with Git on the command line");
    let document = doc.id();
    let submitted = doc.snapshot();
    let request = GitRequest::capture(
        cx.editor,
        commit.workspace().to_owned(),
        GitAction::Commit(commit.snapshot().clone(), doc.text().to_string()),
    )?;
    cx.editor
        .set_status("Preparing commit of the reviewed staged changes…");
    cx.jobs.add(
        Job::with_callback(async move {
            let result = request.prepare().await;
            Ok(Callback::Followup(Box::new(move |editor| {
                let task = result.and_then(|prepared| prepared.admit(editor))?;
                Ok(Some(
                    Job::with_callback(async move {
                        let completion = task.execute().await;
                        Ok(Callback::Editor(Box::new(move |editor| {
                            if completion.succeeded() {
                                editor.finish_commit_message(document, &submitted);
                            }
                            completion.finish(editor);
                        })))
                    })
                    .wait_before_exiting(),
                ))
            })))
        })
        .wait_before_exiting(),
    );
    Ok(())
}

#[cfg(all(test, feature = "integration"))]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn completed_snapshot_does_not_open_a_composer_after_switching_buffers() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "staged\n")?;
        for args in [["init"].as_slice(), ["add", "file.txt"].as_slice()] {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env_remove("GIT_DIR")
                .output()?;
            ensure!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let mut app = crate::ui::picker::test::application(false)?;
        app.editor.open(&path, view::editor::Action::Replace)?;
        let config = crate::config::Config::default();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut jobs = crate::job::Jobs::new();
        {
            let mut cx = compositor::Context {
                config: crate::config::Context {
                    current: &config,
                    updates: &updates,
                },
                editor: &mut app.editor,
                jobs: &mut jobs,
                scroll: None,
                image_picker: None,
                is_cursor_owner: true,
            };
            open(&mut cx, PromptEvent::Validate)?;
        }
        // Leave the read's completion queued until after the user changes buffers.
        let other = app.editor.new_file(view::editor::Action::Replace);
        let callback =
            tokio::time::timeout(std::time::Duration::from_secs(10), jobs.callbacks.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Commit snapshot callback was not delivered"))?;
        let mut compositor = compositor::Compositor::new(view::graphics::Rect::new(0, 0, 80, 24));
        jobs.try_handle_callback(
            crate::config::Context {
                current: &config,
                updates: &updates,
            },
            &mut app.editor,
            &mut compositor,
            Ok(Some(callback)),
        )?;
        ensure!(
            current_ref!(app.editor).1.id() == other,
            "Commit composer stole focus"
        );
        ensure!(
            app.editor
                .documents()
                .all(|doc| doc.commit_message().is_none()),
            "A stale composer was opened"
        );
        ensure!(
            app.close().await.is_empty(),
            "Application did not close cleanly"
        );
        Ok(())
    }
}
