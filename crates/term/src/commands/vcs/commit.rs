//! Commit commands preserve ordinary editing and run mutations to completion.

use crate::{
    compositor,
    job::{Callback, Job},
    ui::PromptEvent,
};
use anyhow::{bail, ensure, Result};
use vcs::operations::GitRepository;
use view::{
    current_ref,
    features::git_operations::{GitAction, GitRequest},
};

pub(super) fn open(cx: &mut compositor::Context, event: PromptEvent) -> Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let (_, doc) = current_ref!(cx.editor);
    let workspace = doc.workspace_root().to_owned();
    let path = doc
        .commit_message()
        .map(|commit| commit.repository().root().to_owned())
        .or_else(|| {
            doc.display_metadata()
                .source_path
                .map(|path| path.to_owned())
        })
        .unwrap_or_else(stdx::env::current_working_dir);
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
