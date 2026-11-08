//! Repository pickers and explicit, editor-coordinated Git commands.
use crate::{
    compositor::{Component, Context},
    job::Callback,
    keymap::component::{PickerAction, PickerItemAction},
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
    repository::{Branch, BranchCursor, BranchPage, Worktree},
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
                    Command::Branches => {
                        Box::new(overlaid(branches(cx.editor, repo, scope.workspace)))
                            as Box<dyn Component>
                    }
                    Command::Worktrees => {
                        Box::new(overlaid(worktrees(cx.editor, repo, scope.workspace)))
                    }
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

fn branches(
    editor: &Editor,
    repo: GitRepository,
    workspace: PathBuf,
) -> Picker<Branch, RepositoryData> {
    let actions = repo.clone();
    let mut pager = BranchPager::new(editor, repo.clone());
    let action_repo = repo.clone();
    let action_workspace = workspace.clone();
    Picker::new_with_callback_result(
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
    .with_title("Git branches")
    .with_help(|maps| {
        super::actions::action_hints(
            &maps.branch_picker,
            &[
                (PickerAction::Accept, "switch"),
                (PickerAction::Item(PickerItemAction::CreateBranch), "create"),
                (PickerAction::Item(PickerItemAction::DeleteBranch), "delete"),
            ],
        )
    })
    .with_item_actions(
        |maps| &maps.branch_picker,
        move |_, branch, action| branch_action(&action_repo, &action_workspace, branch, action),
    )
    .with_editor_updates(move |cx| pager.update(cx))
    .with_item_key(|branch| branch.name.clone())
}

fn worktrees(
    editor: &Editor,
    repo: GitRepository,
    workspace: PathBuf,
) -> Picker<Worktree, RepositoryData> {
    let open = repo.clone();
    let action_repo = repo.clone();
    Picker::new_with_callback_result(
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
    .with_title("Git worktrees")
    .with_help(|maps| {
        super::actions::action_hints(
            &maps.worktree_picker,
            &[
                (PickerAction::Accept, "open"),
                (
                    PickerAction::Item(PickerItemAction::CreateWorktree),
                    "create",
                ),
                (
                    PickerAction::Item(PickerItemAction::RemoveWorktree),
                    "remove",
                ),
            ],
        )
    })
    .with_item_actions(
        |maps| &maps.worktree_picker,
        move |_, tree, action| worktree_action(&action_repo, &workspace, tree, action),
    )
    .with_file_updates(
        editor,
        |data| data.repo.root(),
        |_, _, data, _| Box::pin(async move { data.repo.worktrees().await }),
    )
    .with_refresh_generation(editor, super::actions::generation)
    .with_item_key(|tree| tree.path.display().to_string())
    .refresh_on_vcs_changes()
    .refresh_on_open()
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

fn branch_action(
    repo: &GitRepository,
    workspace: &Path,
    branch: Option<&Branch>,
    action: PickerItemAction,
) -> PickerCallbackResult<Branch, RepositoryData> {
    match action {
        PickerItemAction::CreateBranch => create_prompt(repo, workspace, false),
        PickerItemAction::DeleteBranch => {
            let Some(branch) = branch else {
                return PickerCallbackResult::KeepOpen;
            };
            let repo = repo.clone();
            let workspace = workspace.to_owned();
            let name = branch.name.clone();
            PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, _| {
                compositor.push(super::actions::confirm_branch_deletion(
                    repo, name, workspace,
                ));
            }))
        }
        _ => PickerCallbackResult::KeepOpen,
    }
}

fn worktree_action(
    repo: &GitRepository,
    workspace: &Path,
    tree: Option<&Worktree>,
    action: PickerItemAction,
) -> PickerCallbackResult<Worktree, RepositoryData> {
    match action {
        PickerItemAction::CreateWorktree => create_prompt(repo, workspace, true),
        PickerItemAction::RemoveWorktree => {
            let Some(tree) = tree else {
                return PickerCallbackResult::KeepOpen;
            };
            let repo = repo.clone();
            let workspace = workspace.to_owned();
            let path = tree.path.clone();
            PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, _| {
                compositor.push(super::actions::confirm_worktree_removal(
                    repo, path, workspace,
                ));
            }))
        }
        _ => PickerCallbackResult::KeepOpen,
    }
}

fn create_prompt<T: 'static>(
    repo: &GitRepository,
    workspace: &Path,
    worktree: bool,
) -> PickerCallbackResult<T, RepositoryData> {
    let repo = repo.clone();
    let workspace = workspace.to_owned();
    PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, _| {
        compositor.push(Box::new(ui::Prompt::new_with_callback(
            if worktree {
                "New worktree path: "
            } else {
                "New branch from HEAD: "
            }
            .into(),
            None,
            ui::completers::none,
            move |cx, input, event| {
                if event != ui::PromptEvent::Validate || input.trim().is_empty() {
                    return None;
                }
                if worktree {
                    let path = stdx::path::expand_tilde(Path::new(input));
                    let path = stdx::path::canonicalize_existing(&repo.root().join(path));
                    let repo = repo.clone();
                    let workspace = workspace.clone();
                    return Some(Box::new(move |compositor, _| {
                        compositor.push(Box::new(ui::Prompt::new(
                            "Existing branch (empty for detached HEAD): ".into(),
                            None,
                            ui::completers::none,
                            move |cx, input, event| {
                                if event == ui::PromptEvent::Validate {
                                    super::actions::start(
                                        cx,
                                        workspace.clone(),
                                        GitAction::Repository(
                                            repo.clone(),
                                            RepositoryOperation::CreateWorktree {
                                                path: path.clone(),
                                                branch: (!input.is_empty())
                                                    .then(|| input.to_owned()),
                                            },
                                        ),
                                    );
                                }
                            },
                        )));
                    }));
                }
                let operation = RepositoryOperation::CreateBranch {
                    name: input.to_owned(),
                };
                super::actions::start(
                    cx,
                    workspace.clone(),
                    GitAction::Repository(repo.clone(), operation),
                );
                None
            },
        )));
    }))
}

struct BranchPager {
    repo: GitRepository,
    cursor: Option<BranchCursor>,
    pending: Option<tokio::sync::oneshot::Receiver<Result<BranchPage>>>,
    rows: Vec<Branch>,
    failed: bool,
    replacing: bool,
    replacement: Vec<Branch>,
    dirty: bool,
    changes: view::features::workspace_changes::WorkspaceChanges,
    generation: u64,
}

impl BranchPager {
    fn new(editor: &Editor, repo: GitRepository) -> Self {
        use view::features::workspace_changes::{Policy, WorkspaceChanges};
        Self {
            cursor: Some(repo.branch_cursor()),
            changes: WorkspaceChanges::new(
                editor,
                repo.root(),
                Policy {
                    vcs: true,
                    ignore_files: true,
                    ..Policy::default()
                },
            ),
            generation: super::actions::generation(editor),
            repo,
            pending: None,
            rows: Vec::new(),
            failed: false,
            replacing: true,
            replacement: Vec::new(),
            dirty: false,
        }
    }

    fn update(&mut self, cx: crate::ui::picker::UpdateContext<'_, Branch>) -> Option<Vec<Branch>> {
        let generation = super::actions::generation(cx.editor);
        self.dirty |= self.changes.changed(cx.editor, self.repo.root());
        // Let an in-flight page publish before starting a queued filesystem refresh.
        if cx.resumed
            || generation != self.generation
            || (self.pending.is_none() && (self.dirty || self.changes.poll_due(cx.editor)))
        {
            cx.task.cancel();
            self.pending = None;
            self.cursor = Some(self.repo.branch_cursor());
            self.failed = false;
            self.replacing = true;
            self.replacement.clear();
            self.dirty = false;
            self.generation = generation;
            self.changes.mark_refreshed();
        }
        let mut changed = false;
        if let Some(pending) = &mut self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(page) => {
                            self.cursor = page.next;
                            self.changes.mark_refreshed();
                            if self.replacing {
                                self.replacement.extend(page.branches);
                                // Refresh the previously visible prefix together so polling
                                // cannot remove the selected row merely by restarting at page one.
                                if self.replacement.len() >= self.rows.len()
                                    || self.cursor.is_none()
                                {
                                    self.rows = std::mem::take(&mut self.replacement);
                                    self.replacing = false;
                                    changed = true;
                                }
                            } else {
                                self.rows.extend(page.branches);
                                changed = true;
                            }
                            self.rows.sort_by(|a, b| a.name.cmp(&b.name));
                        }
                        Err(error) => {
                            self.failed = true;
                            cx.editor.set_error(|| error.to_string());
                        }
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.pending = None;
                    self.failed = true;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if !changed
            && self.pending.is_none()
            && !self.failed
            && (self.replacing
                || self.rows.is_empty()
                || !cx.query.trim().is_empty()
                || cx.near_end)
            && let Some(cursor) = self.cursor.clone()
        {
            let cancel = cx.task.restart();
            let redraw = cx.editor.redraw().clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.pending = Some(rx);
            tokio::spawn(async move {
                if let Some(result) = event::cancelable_future(cursor.next_page(), cancel).await {
                    let _ = tx.send(result);
                    redraw.request();
                }
            });
        }
        *cx.incomplete = self.cursor.is_some() || self.failed;
        *cx.status = Some(
            if self.failed {
                "Branches incomplete; reopen to retry"
            } else if self.pending.is_some() {
                "Loading branches…"
            } else if self.cursor.is_some() {
                "More branches available"
            } else {
                "Complete branch list"
            }
            .into(),
        );
        changed.then(|| self.rows.clone())
    }
}

#[cfg(test)]
mod tests;
