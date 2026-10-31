//! Thin terminal entry points for repository-wide review snapshots.

use anyhow::{bail, Result};
use command_line::Args;
#[cfg(feature = "git")]
use view::editor::Action;
use view::features::change_review::ReviewComparison;

use crate::{compositor, ui::PromptEvent};

pub(super) fn open(cx: &mut compositor::Context, args: Args, event: PromptEvent) -> Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let comparison = match args.first() {
        None | Some("buffer") => ReviewComparison::Buffer,
        Some("staged") => ReviewComparison::Staged,
        Some("unstaged") => ReviewComparison::Unstaged,
        _ => bail!("Expected staged, unstaged, or buffer"),
    };
    let (_, doc) = view::current_ref!(cx.editor);
    let path = doc
        .path()
        .or_else(|| doc.review().map(|r| r.path()))
        .or_else(|| doc.project_review().map(|r| r.root.as_path()))
        .map(std::path::Path::to_owned)
        .unwrap_or_else(stdx::env::current_working_dir);
    let workspace = doc
        .project_review()
        .map_or_else(|| doc.workspace_root().to_owned(), |r| r.workspace.clone());
    #[cfg(not(feature = "git"))]
    {
        let _ = (path, workspace, comparison);
        bail!("No Git support included in this build");
    }
    #[cfg(feature = "git")]
    {
        let origin = doc.id();
        cx.jobs.callback(async move {
            let root = tokio::task::spawn_blocking(move || {
                vcs::operations::GitRepository::discover(&path).map(|repo| repo.root().to_owned())
            })
            .await?;
            Ok(crate::job::Callback::Editor(Box::new(move |editor| {
                if view::current_ref!(editor).1.id() != origin {
                    return;
                }
                match root {
                    Ok(root) => {
                        editor.open_project_review(root, workspace, comparison, Action::Replace);
                    }
                    Err(error) => editor.set_error(|| error.to_string()),
                }
            })))
        });
        Ok(())
    }
}

pub(super) fn refresh(cx: &mut compositor::Context, _: Args, event: PromptEvent) -> Result<()> {
    if event == PromptEvent::Validate {
        cx.editor.refresh_project_review()?;
    }
    Ok(())
}

pub(super) fn navigate(cx: &mut compositor::Context, args: Args, event: PromptEvent) -> Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let (forward, files) = match args.first() {
        None | Some("hunk") => (true, false),
        Some("previous-hunk") => (false, false),
        Some("file") => (true, true),
        Some("previous-file") => (false, true),
        _ => bail!("Expected file, previous-file, hunk, or previous-hunk"),
    };
    cx.editor.navigate_project_review(forward, files)
}

pub(super) fn open_file(cx: &mut compositor::Context, _: Args, event: PromptEvent) -> Result<()> {
    if event == PromptEvent::Validate {
        cx.editor.open_project_review_file()?;
    }
    Ok(())
}
