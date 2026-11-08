//! Repository pickers and explicit, editor-coordinated Git commands.
use crate::{
    compositor::{Component, Context},
    job::Callback,
    ui::{self, overlay::overlaid, picker::PickerCallbackResult, Picker, PickerColumn},
};
use anyhow::{ensure, Result};
use command_line::Args;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tui::text::Span;
use vcs::operations::{
    repository::{Branch, Worktree},
    GitRepository, RepositoryOperation,
};
use view::{features::git_operations::GitAction, Editor};

pub(super) enum Command {
    Branches,
    BranchCreate,
    BranchSwitch,
    Worktrees,
    WorktreeCreate,
    WorktreeOpen,
    WorktreeRemove,
    Fetch,
    Pull,
    Push,
}

fn argument_path(value: &str) -> PathBuf {
    stdx::path::canonicalize_existing(&stdx::path::expand_tilde(Path::new(value)))
}

enum Request {
    Picker(Command),
    Operation(RepositoryOperation),
    Open(PathBuf),
    Remove(PathBuf),
}

pub(super) fn execute(cx: &mut Context, args: Args, command: Command) -> Result<()> {
    let scope = view::features::git_operations::GitScope::capture(cx.editor);
    let request = match command {
        Command::Branches | Command::Worktrees => Request::Picker(command),
        Command::BranchCreate => Request::Operation(RepositoryOperation::CreateBranch {
            name: args[0].to_owned(),
        }),
        Command::BranchSwitch => Request::Operation(RepositoryOperation::SwitchBranch {
            name: args[0].to_owned(),
        }),
        Command::WorktreeCreate => Request::Operation(RepositoryOperation::CreateWorktree {
            path: argument_path(&args[0]),
            branch: args.get(1).map(|name| name.to_string()),
        }),
        Command::WorktreeRemove => {
            let path = argument_path(&args[0]);
            ensure!(
                !stdx::path::canonicalize_existing(&stdx::env::current_working_dir())
                    .starts_with(&path),
                "Change directory out of this worktree before removing it"
            );
            Request::Remove(path)
        }
        Command::WorktreeOpen => Request::Open(argument_path(&args[0])),
        Command::Fetch => Request::Operation(RepositoryOperation::Fetch {
            remote: args.first().map(str::to_owned),
        }),
        Command::Pull => Request::Operation(RepositoryOperation::Pull {
            remote: args.first().map(str::to_owned),
        }),
        Command::Push => Request::Operation(RepositoryOperation::Push {
            remote: args.first().map(str::to_owned),
        }),
    };
    cx.editor.set_status("Reading Git repository…");
    cx.jobs.callback(async move {
        let repo =
            tokio::task::spawn_blocking(move || GitRepository::discover(&scope.path)).await??;
        Ok(Callback::ui(move |compositor, cx| match request {
            Request::Picker(command) => {
                let picker = match command {
                    Command::Branches => branches(cx.editor, repo, scope.workspace),
                    Command::Worktrees => worktrees(cx.editor, repo),
                    _ => unreachable!("only browse commands create a picker"),
                };
                compositor.push(picker);
            }
            Request::Operation(operation) => {
                super::actions::start(cx, scope.workspace, GitAction::Repository(repo, operation))
            }
            Request::Remove(path) => compositor.push(super::actions::confirm_worktree_removal(
                repo,
                path,
                scope.workspace,
            )),
            Request::Open(path) => request_open_worktree(cx, repo, path),
        }))
    });
    Ok(())
}

struct RepositoryData {
    repo: GitRepository,
    theme: Arc<theme::Theme>,
}

impl RepositoryData {
    fn new(editor: &Editor, repo: GitRepository) -> Self {
        Self {
            repo,
            theme: Arc::new(editor.theme.clone()),
        }
    }
}

fn branches(editor: &Editor, repo: GitRepository, workspace: PathBuf) -> Box<dyn Component> {
    let actions = repo.clone();
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("branch", |branch: &Branch, data: &RepositoryData| {
                Span::styled(
                    branch.name.as_str(),
                    data.theme.get(if branch.current {
                        "diff.plus"
                    } else {
                        "string.special.symbol"
                    }),
                )
                .into()
            }),
            PickerColumn::new("current", |branch: &Branch, data| {
                Span::styled(
                    if branch.current { "*" } else { "" },
                    data.theme.get("diff.plus"),
                )
                .into()
            }),
            PickerColumn::new("upstream", |branch: &Branch, data| {
                Span::styled(branch.upstream.as_str(), data.theme.get("string.special")).into()
            }),
        ],
        0,
        Vec::<Branch>::new(),
        RepositoryData::new(editor, repo),
        move |_, branch, _| {
            let action = GitAction::Repository(
                actions.clone(),
                RepositoryOperation::SwitchBranch {
                    name: branch.name.clone(),
                },
            );
            let workspace = workspace.clone();
            PickerCallbackResult::CloseWithCallback(Box::new(move |_, cx| {
                super::actions::start(cx, workspace, action)
            }))
        },
    )
    .with_title("Git branches · Enter switches branch")
    .with_file_updates(
        editor,
        |data| data.repo.root(),
        |_, _, data, _| Box::pin(async move { data.repo.branches().await }),
    )
    .with_refresh_generation(editor, super::actions::generation)
    .with_item_key(|branch| branch.name.clone())
    .refresh_on_vcs_changes()
    .refresh_on_open();
    Box::new(overlaid(picker))
}

fn worktrees(editor: &Editor, repo: GitRepository) -> Box<dyn Component> {
    let open = repo.clone();
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("path", |tree: &Worktree, data: &RepositoryData| {
                Span::styled(
                    tree.path.display().to_string(),
                    data.theme.get("ui.text.directory"),
                )
                .into()
            }),
            PickerColumn::new("branch", |tree: &Worktree, data| {
                Span::styled(
                    tree.branch.as_deref().unwrap_or("(detached)"),
                    data.theme.get(if tree.branch.is_some() {
                        "string.special.symbol"
                    } else {
                        "ui.text.inactive"
                    }),
                )
                .into()
            }),
            PickerColumn::new("state", |tree: &Worktree, data| {
                let (label, scope) = if tree.locked {
                    ("locked", "warning")
                } else if tree.prunable {
                    ("unavailable", "error")
                } else if tree.main {
                    ("main", "diff.plus")
                } else {
                    ("linked", "ui.text.inactive")
                };
                Span::styled(label, data.theme.get(scope)).into()
            }),
        ],
        0,
        Vec::<Worktree>::new(),
        RepositoryData::new(editor, repo),
        move |_, tree, _| {
            let path = tree.path.clone();
            let repo = open.clone();
            PickerCallbackResult::CloseWithCallback(Box::new(move |_, cx| {
                request_open_worktree(cx, repo, path);
            }))
        },
    )
    .with_title("Git worktrees · Enter opens file picker")
    .with_file_updates(
        editor,
        |data| data.repo.root(),
        |_, _, data, _| Box::pin(async move { data.repo.worktrees().await }),
    )
    .with_refresh_generation(editor, super::actions::generation)
    .with_item_key(|tree| tree.path.display().to_string())
    .refresh_on_vcs_changes()
    .refresh_on_open();
    Box::new(overlaid(picker))
}

fn request_open_worktree(cx: &mut Context, repo: GitRepository, path: PathBuf) {
    cx.jobs.callback(async move {
        let trees = repo.worktrees().await?;
        ensure!(
            trees.iter().any(|tree| tree.path == path && !tree.prunable),
            "Choose a registered, available worktree path"
        );
        let path = tokio::task::spawn_blocking(move || {
            let target = GitRepository::discover(&path)?;
            ensure!(
                target.root() == path && target.common_dir() == repo.common_dir(),
                "Selected path is no longer a worktree of this repository"
            );
            Ok::<_, anyhow::Error>(path)
        })
        .await??;
        Ok(Callback::ui(move |compositor, cx| {
            let editor = &mut *cx.editor;
            match editor.set_cwd(&path) {
                Ok(()) => compositor.push(Box::new(overlaid(ui::file_picker(editor, path)))),
                Err(error) => editor.set_error(|| error.to_string()),
            }
        }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::picker::test::{application, frame, until};
    use view::graphics::Color;

    fn assert_colour(surface: &tui::buffer::Buffer, text: &str, colour: Color) {
        // Skip the border, query, separator, and column headings.
        for y in 4..surface.area.height {
            let row: String = (0..surface.area.width)
                .map(|x| surface[(x, y)].symbol())
                .collect();
            if let Some(offset) = row.find(text) {
                let offset = row[..offset].chars().count();
                for x in offset..offset + text.len() {
                    assert_eq!(
                        surface[(x as u16, y)].fg,
                        colour,
                        "{text}: row {y} column {x}: {row}"
                    );
                }
                return;
            }
        }
        panic!("Missing picker text: {text}");
    }

    #[tokio::test]
    async fn repository_pickers_preserve_semantic_colours_on_selected_rows() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let output = std::process::Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(dir.path())
            .output()?;
        ensure!(output.status.success(), "git init failed");
        std::fs::write(
            dir.path().join(".git/refs/heads/main"),
            "1111111111111111111111111111111111111111\n",
        )?;
        let repo = GitRepository::discover(dir.path())?;
        let mut app = application(false)?;
        app.editor.theme = theme::Theme::from(toml::Value::Table(toml::toml! {
            "ui.text" = "white"
            "ui.text.directory" = "blue"
            "ui.text.focus" = { modifiers = ["bold"] }
            "diff.plus" = "green"
            "string.special.symbol" = "cyan"
            "ui.selection" = { bg = "black" }
            "ui.highlight" = { bg = "black" }
            "special" = { modifiers = ["bold"] }
        }));
        let mut branches = branches(&app.editor, repo.clone(), dir.path().into())
            .as_boxed_any()
            .downcast::<ui::overlay::Overlay<Picker<Branch, RepositoryData>>>()
            .map_err(|_| anyhow::anyhow!("Expected branch picker"))?;
        until(&mut branches.content, &mut app, |p| p.selection().is_some()).await?;
        let surface = frame(&mut branches.content, &mut app);
        assert_colour(&surface, "main", Color::Green);
        assert_colour(&surface, "*", Color::Green);
        let mut worktrees = worktrees(&app.editor, repo)
            .as_boxed_any()
            .downcast::<ui::overlay::Overlay<Picker<Worktree, RepositoryData>>>()
            .map_err(|_| anyhow::anyhow!("Expected worktree picker"))?;
        until(&mut worktrees.content, &mut app, |p| {
            p.selection().is_some()
        })
        .await?;
        let surface = frame(&mut worktrees.content, &mut app);
        assert_colour(&surface, std::path::MAIN_SEPARATOR_STR, Color::Blue);
        Ok(())
    }
}
