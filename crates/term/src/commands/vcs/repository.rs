//! Repository pickers and explicit, editor-coordinated Git commands.
use crate::{
    compositor::{Component, Context},
    job::Callback,
    ui::{self, overlay::overlaid, picker::PickerCallbackResult, Picker, PickerColumn},
};
use anyhow::{ensure, Result};
use command_line::Args;
use std::path::{Path, PathBuf};
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
    Remotes,
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
        Command::Branches | Command::Worktrees | Command::Remotes => Request::Picker(command),
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
        let (repo, request, names) = tokio::task::spawn_blocking(move || {
            let repo = GitRepository::discover(&scope.path)?;
            let names = if matches!(&request, Request::Picker(Command::Remotes)) {
                Some(repo.remote_names()?)
            } else {
                None
            };
            Ok::<_, anyhow::Error>((repo, request, names))
        })
        .await??;
        Ok(Callback::Context(Box::new(
            move |compositor, cx| match request {
                Request::Picker(command) => {
                    let picker = match command {
                        Command::Branches => branches(cx.editor, repo, scope.workspace),
                        Command::Worktrees => worktrees(cx.editor, repo),
                        Command::Remotes => {
                            remotes(cx.editor, repo, scope.workspace, names.unwrap_or_default())
                        }
                        _ => unreachable!("only browse commands create a picker"),
                    };
                    compositor.push(picker);
                }
                Request::Operation(operation) => super::actions::start(
                    cx,
                    scope.workspace,
                    GitAction::Repository(repo, operation),
                ),
                Request::Remove(path) => compositor.push(super::actions::confirm_worktree_removal(
                    repo,
                    path,
                    scope.workspace,
                )),
                Request::Open(path) => request_open_worktree(cx, repo, path),
            },
        )))
    });
    Ok(())
}

fn branches(editor: &Editor, repo: GitRepository, workspace: PathBuf) -> Box<dyn Component> {
    let actions = repo.clone();
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("branch", |branch: &Branch, _: &GitRepository| {
                branch.name.as_str().into()
            }),
            PickerColumn::new("current", |branch: &Branch, _| {
                if branch.current { "*" } else { "" }.into()
            }),
            PickerColumn::new("upstream", |branch: &Branch, _| {
                branch.upstream.as_str().into()
            }),
        ],
        0,
        Vec::<Branch>::new(),
        repo,
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
    .with_file_updates(editor, GitRepository::root, |_, _, repo, _| {
        Box::pin(async move { repo.branches().await })
    })
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
            PickerColumn::new("path", |tree: &Worktree, _: &GitRepository| {
                tree.path.display().to_string().into()
            }),
            PickerColumn::new("branch", |tree: &Worktree, _| {
                tree.branch.as_deref().unwrap_or("(detached)").into()
            }),
            PickerColumn::new("state", |tree: &Worktree, _| {
                if tree.locked {
                    "locked"
                } else if tree.prunable {
                    "unavailable"
                } else if tree.main {
                    "main"
                } else {
                    "linked"
                }
                .into()
            }),
        ],
        0,
        Vec::<Worktree>::new(),
        repo,
        move |_, tree, _| {
            let path = tree.path.clone();
            let repo = open.clone();
            PickerCallbackResult::CloseWithCallback(Box::new(move |_, cx| {
                request_open_worktree(cx, repo, path);
            }))
        },
    )
    .with_title("Git worktrees · Enter opens file picker")
    .with_file_updates(editor, GitRepository::root, |_, _, repo, _| {
        Box::pin(async move { repo.worktrees().await })
    })
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
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| match editor.set_cwd(&path) {
                Ok(()) => compositor.push(Box::new(overlaid(ui::file_picker(editor, path)))),
                Err(error) => editor.set_error(|| error.to_string()),
            },
        )))
    });
}

fn remotes(
    editor: &Editor,
    repo: GitRepository,
    workspace: PathBuf,
    names: Vec<String>,
) -> Box<dyn Component> {
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        [PickerColumn::new("remote", |name: &String, _: &()| {
            name.as_str().into()
        })],
        0,
        names,
        (),
        move |_, remote, _| {
            let remote = remote.clone();
            let repo = repo.clone();
            let workspace = workspace.clone();
            PickerCallbackResult::CloseWithCallback(Box::new(move |compositor, cx| {
                compositor.push(Box::new(overlaid(remote_actions(
                    cx.editor, repo, workspace, remote,
                ))));
            }))
        },
    )
    .with_title("Git remotes · choose an operation");
    Box::new(overlaid(picker))
}

fn remote_actions(
    editor: &Editor,
    repo: GitRepository,
    workspace: PathBuf,
    remote: String,
) -> Picker<RepositoryOperation, ()> {
    let title = format!("Remote {remote}");
    let operations = vec![
        RepositoryOperation::Fetch {
            remote: Some(remote.clone()),
        },
        RepositoryOperation::Pull {
            remote: Some(remote.clone()),
        },
        RepositoryOperation::Push {
            remote: Some(remote),
        },
    ];
    Picker::new_with_callback_result(
        editor.redraw().clone(),
        [PickerColumn::new(
            "action",
            |operation: &RepositoryOperation, _: &()| operation.label().into(),
        )],
        0,
        operations,
        (),
        move |_, operation, _| {
            let action = GitAction::Repository(repo.clone(), operation.clone());
            let workspace = workspace.clone();
            PickerCallbackResult::CloseWithCallback(Box::new(move |_, cx| {
                super::actions::start(cx, workspace, action)
            }))
        },
    )
    .with_title(title)
}
