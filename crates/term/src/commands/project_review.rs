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
    let combined = args.first().is_none() || args.first() == Some("buffer");
    let comparison = match args.first() {
        None => ReviewComparison::Unstaged,
        Some("buffer") => ReviewComparison::Buffer,
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
        let _ = (path, workspace, comparison, combined);
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
                        if combined {
                            editor.open_git_review(
                                root,
                                workspace,
                                comparison == ReviewComparison::Buffer,
                                Action::Replace,
                            );
                        } else {
                            editor.open_project_review(
                                root,
                                workspace,
                                comparison,
                                Action::Replace,
                            );
                        }
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

pub(crate) fn action(
    cx: &mut super::Context,
    action: crate::keymap::component::ProjectReviewAction,
) -> Result<()> {
    use crate::keymap::component::ProjectReviewAction as A;
    match action {
        A::Commit => {
            let mut context = compositor::Context {
                config: cx.config,
                editor: cx.editor,
                jobs: cx.jobs,
                scroll: None,
                image_picker: None,
                is_cursor_owner: false,
            };
            super::vcs::typed::commit(&mut context, Args::empty(), PromptEvent::Validate)?;
        }
        A::Stage | A::Unstage => {
            #[cfg(feature = "git")]
            {
                let kind = if action == A::Stage {
                    vcs::operations::FileOperation::Stage
                } else {
                    vcs::operations::FileOperation::Unstage
                };
                let captured = view::features::git_operations::partial::capture_project_action(
                    cx.editor, kind,
                )?;
                cx.editor.enter_normal_mode();
                super::vcs::partial::start_captured(cx.editor, cx.jobs, captured, true);
            }
            #[cfg(not(feature = "git"))]
            bail!("No Git support included in this build");
        }
        A::ToggleFold => {
            let (view, doc) = view::current!(cx.editor);
            doc.fold(view.id, view::document::FoldAction::Toggle);
        }
        A::ToggleBuffers => cx.editor.toggle_project_review_buffers()?,
        A::OpenSource => cx.editor.open_project_review_source()?,
        A::OpenReview => cx.editor.open_project_review_file()?,
        A::Refresh => cx.editor.refresh_project_review()?,
    }
    Ok(())
}

pub(super) fn open_default(cx: &mut super::Context) {
    let mut context = compositor::Context {
        config: cx.config,
        editor: cx.editor,
        jobs: cx.jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: false,
    };
    if let Err(error) = open(&mut context, Args::empty(), PromptEvent::Validate) {
        context.editor.set_error(|| error.to_string());
    }
}
