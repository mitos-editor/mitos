//! Terminal presentation for backend-owned Git workflows.

use crate::{
    compositor::{self, Component, Context, Event, EventResult},
    job::{Callback, Job},
    keymap::component::PickerItemAction,
    ui::{
        self,
        menu::{Item, Row},
        overlay::overlaid,
        picker::PickerCallbackResult,
        Picker, PickerColumn, PromptEvent,
    },
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tui::text::Span;
use vcs::operations::{FileOperation, GitRepository, StashEntry, StashFile};
use view::{
    features::git_operations::{GitAction, GitRequest},
    Editor,
};

#[derive(Clone)]
enum Command {
    Cancel,
    Run(GitAction),
    Confirmed(GitAction),
    Stashes(PathBuf),
}

#[derive(Clone)]
struct Choice {
    label: String,
    command: Command,
}
impl Choice {
    fn new(label: impl Into<String>, command: Command) -> Self {
        Self {
            label: label.into(),
            command,
        }
    }
}
impl Item for Choice {
    type Data = ();
    fn format(&self, _: &()) -> Row<'_> {
        Row::new([self.label.as_str()])
    }
}

struct ActionMenu {
    select: ui::Select<Choice>,
    selected: Arc<Mutex<Option<Command>>>,
    workspace: PathBuf,
}

impl ActionMenu {
    fn new(message: String, choices: Vec<Choice>, workspace: PathBuf) -> Self {
        let selected = Arc::new(Mutex::new(None));
        let destination = selected.clone();
        Self {
            select: ui::Select::new(message, choices, (), move |_, choice: &Choice, event| {
                if event == PromptEvent::Validate {
                    *destination.lock().unwrap() = Some(choice.command.clone());
                }
            }),
            selected,
            workspace,
        }
    }
}
impl Component for ActionMenu {
    fn owns_cursor(&self) -> bool {
        true
    }
    fn has_pending_keys(&self) -> bool {
        self.select.has_pending_keys()
    }
    fn required_size(&mut self, size: (u16, u16)) -> Option<(u16, u16)> {
        self.select.required_size(size)
    }
    fn render(
        &mut self,
        area: view::graphics::Rect,
        surface: &mut tui::buffer::Buffer,
        cx: &mut Context,
    ) {
        self.select.render(area, surface, cx);
    }
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let result = self.select.handle_event(event, cx);
        let Some(command) = self.selected.lock().unwrap().take() else {
            return result;
        };
        let workspace = self.workspace.clone();
        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            compositor.pop();
            dispatch(compositor, cx, workspace, command);
        })))
    }
}

fn menu(message: String, choices: Vec<Choice>, workspace: PathBuf) -> Box<dyn Component> {
    Box::new(ActionMenu::new(message, choices, workspace))
}

fn dispatch(
    compositor: &mut compositor::Compositor,
    cx: &mut Context,
    workspace: PathBuf,
    command: Command,
) {
    match command {
        Command::Cancel => {}
        Command::Stashes(path) => match stash_browser(cx.editor, path, workspace) {
            Ok(picker) => compositor.push(picker),
            Err(error) => cx.editor.set_error(|| error.to_string()),
        },
        Command::Run(action @ (GitAction::File(..) | GitAction::Partial { .. }))
        | Command::Confirmed(action) => start(cx, workspace, action),
        Command::Run(action) => {
            let (message, label) = match &action {
                GitAction::Stash(path) => (
                    format!(
                        "Stash all changes in the repository containing:\n{}\n\
                        Includes staged, unstaged and untracked files.\n\
                        Save or close modified buffers first; ignored files are excluded.",
                        path.display()
                    ),
                    "Stash all repository changes",
                ),
                GitAction::Apply(repo, entry, index) => (
                    format!(
                        "Apply {} ({})?\nRepository: {}\n\
                        Restore staged changes: {}\n\
                        The stash will be kept, including if Git reports conflicts.",
                        entry.selector,
                        &entry.id()[..12],
                        repo.root().display(),
                        if *index { "yes" } else { "no" }
                    ),
                    "Apply and keep stash",
                ),
                GitAction::File(..) | GitAction::Partial { .. } => unreachable!(),
            };
            compositor.push(menu(
                message,
                vec![
                    Choice::new("Cancel", Command::Cancel),
                    Choice::new(label, Command::Confirmed(action)),
                ],
                workspace,
            ));
        }
    }
}

fn start(cx: &mut Context, workspace: PathBuf, action: GitAction) {
    let request = match GitRequest::capture(cx.editor, workspace, action) {
        Ok(request) => request,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    cx.editor.set_status("Preparing Git action…");
    cx.jobs.add(
        Job::with_callback(async move {
            let result = request.prepare().await;
            Ok(Callback::Followup(Box::new(move |editor| {
                let task = result.and_then(|prepared| prepared.admit(editor))?;
                Ok(Some(
                    Job::with_callback(async move {
                        let completion = task.execute().await;
                        Ok(Callback::Editor(Box::new(move |editor| {
                            completion.finish(editor)
                        })))
                    })
                    .wait_before_exiting(),
                ))
            })))
        })
        .wait_before_exiting(),
    );
}

pub(super) fn file_action<T, D>(
    cx: &mut Context,
    status: Option<&vcs::FileStatus>,
    repository_root: Option<&Path>,
    action: PickerItemAction,
    scope: &vcs::ChangedFileScope,
    workspace: &Path,
) -> PickerCallbackResult<T, D> {
    let path = repository_root.unwrap_or(scope.path()).to_owned();
    let workspace = workspace.to_owned();
    let command = match action {
        PickerItemAction::Stage | PickerItemAction::Unstage => {
            let Some(status) = status else {
                return PickerCallbackResult::KeepOpen;
            };
            Command::Run(GitAction::File(
                status.clone(),
                if action == PickerItemAction::Stage {
                    FileOperation::Stage
                } else {
                    FileOperation::Unstage
                },
            ))
        }
        PickerItemAction::Stash => Command::Run(GitAction::Stash(path)),
        PickerItemAction::Stashes => Command::Stashes(path),
        PickerItemAction::Actions => {
            let mut choices = Vec::new();
            if let Some(status) = status {
                if status.unstaged.is_some() {
                    choices.push(Choice::new(
                        "Stage selected file on disk",
                        Command::Run(GitAction::File(status.clone(), FileOperation::Stage)),
                    ));
                }
                if status.staged.is_some() {
                    choices.push(Choice::new(
                        "Unstage selected file",
                        Command::Run(GitAction::File(status.clone(), FileOperation::Unstage)),
                    ));
                }
            }
            choices.push(Choice::new(
                "Stash all repository changes…",
                Command::Run(GitAction::Stash(path.clone())),
            ));
            choices.push(Choice::new("Browse stashes…", Command::Stashes(path)));
            return PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, _| {
                compositor.push(menu("Git actions".into(), choices, workspace));
            }));
        }
        _ => return PickerCallbackResult::KeepOpen,
    };
    // Stage/unstage start immediately, leaving the query and selected row intact.
    if let Command::Run(action @ GitAction::File(..)) = command {
        start(cx, workspace, action);
        return PickerCallbackResult::KeepOpen;
    }
    PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, cx| {
        dispatch(compositor, cx, workspace, command)
    }))
}

pub(super) fn file_help(
    keys: &ui_core::keymap::Keymap<crate::keymap::component::PickerAction>,
) -> Vec<String> {
    use crate::keymap::component::PickerAction as P;
    action_hints(
        keys,
        &[
            (P::Accept, "open"),
            (P::Item(PickerItemAction::Stage), "stage"),
            (P::Item(PickerItemAction::Unstage), "unstage"),
            (P::Item(PickerItemAction::Stash), "stash"),
            (P::Item(PickerItemAction::Stashes), "stashes"),
            (P::Item(PickerItemAction::Actions), "actions"),
        ],
    )
}

fn stash_help(
    keys: &ui_core::keymap::Keymap<crate::keymap::component::PickerAction>,
) -> Vec<String> {
    use crate::keymap::component::PickerAction as P;
    action_hints(
        keys,
        &[
            (P::Accept, "inspect"),
            (P::Item(PickerItemAction::ApplyStash), "apply"),
            (P::Item(PickerItemAction::Actions), "actions"),
            (
                P::Item(PickerItemAction::ApplyStashWithIndex),
                "apply with staged changes",
            ),
        ],
    )
}

pub(super) fn action_hints(
    keys: &ui_core::keymap::Keymap<crate::keymap::component::PickerAction>,
    actions: &[(crate::keymap::component::PickerAction, &str)],
) -> Vec<String> {
    actions
        .iter()
        .filter_map(|(action, label)| {
            let key = crate::keymap::component::action_hint(keys, action);
            if key == "unbound" {
                None
            } else {
                Some(format!("{key} {label}"))
            }
        })
        .collect()
}

pub(super) fn generation(editor: &Editor) -> u64 {
    editor.handlers().git_operations.revision()
}

pub(super) fn stash_browser(
    editor: &Editor,
    path: PathBuf,
    workspace: PathBuf,
) -> anyhow::Result<Box<dyn Component>> {
    let repo = GitRepository::discover(&path)?;
    Ok(Box::new(overlaid(
        stash_picker(editor, repo, workspace, Vec::new()).refresh_on_open(),
    )))
}

struct StashData {
    repo: GitRepository,
    theme: Arc<theme::Theme>,
}
fn stash_picker(
    editor: &Editor,
    repo: GitRepository,
    workspace: PathBuf,
    entries: Vec<StashEntry>,
) -> Picker<StashEntry, StashData> {
    let inspect_repo = repo.clone();
    let action_repo = repo.clone();
    let action_workspace = workspace.clone();
    Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("stash", |entry: &StashEntry, data: &StashData| {
                Span::styled(
                    entry.selector.as_str(),
                    data.theme.get("string.special.symbol"),
                )
                .into()
            }),
            PickerColumn::new("commit", |entry: &StashEntry, data| {
                Span::styled(&entry.id()[..12], data.theme.get("constant")).into()
            }),
            PickerColumn::new("message", |entry: &StashEntry, _| {
                entry.message.as_str().into()
            }),
        ],
        2,
        entries,
        StashData {
            repo,
            theme: Arc::new(editor.theme.clone()),
        },
        move |_, entry, _| {
            let repo = inspect_repo.clone();
            let entry = entry.clone();
            PickerCallbackResult::CloseWithCallback(Box::new(move |compositor, cx| {
                compositor.push(Box::new(overlaid(stash_files_picker(
                    cx.editor, repo, entry,
                ))));
            }))
        },
    )
    .with_help(|maps| stash_help(&maps.stash_picker))
    .with_file_updates(
        editor,
        |data| data.repo.root(),
        |_, _, data, _| Box::pin(async move { data.repo.stashes().await }),
    )
    .with_refresh_generation(editor, generation)
    .with_item_key(|entry| entry.id().to_owned())
    .refresh_on_vcs_changes()
    .with_item_actions(
        |maps| &maps.stash_picker,
        move |_, entry, action| {
            let Some(entry) = entry else {
                return PickerCallbackResult::KeepOpen;
            };
            let apply =
                |index| Command::Run(GitAction::Apply(action_repo.clone(), entry.clone(), index));
            let workspace = action_workspace.clone();
            let command = match action {
                PickerItemAction::ApplyStash => apply(false),
                PickerItemAction::ApplyStashWithIndex => apply(true),
                PickerItemAction::Actions => {
                    let choices = vec![
                        Choice::new("Apply and keep stash…", apply(false)),
                        Choice::new("Apply, restore staged changes and keep stash…", apply(true)),
                    ];
                    return PickerCallbackResult::KeepOpenWithCallback(Box::new(
                        move |compositor, _| {
                            compositor.push(menu("Stash actions".into(), choices, workspace))
                        },
                    ));
                }
                _ => return PickerCallbackResult::KeepOpen,
            };
            PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, cx| {
                dispatch(compositor, cx, workspace, command)
            }))
        },
    )
}

fn stash_files_picker(
    editor: &Editor,
    repo: GitRepository,
    entry: StashEntry,
) -> Picker<StashFile, StashFiles> {
    let data = StashFiles {
        repo: repo.clone(),
        entry: entry.clone(),
        theme: Arc::new(editor.theme.clone()),
    };
    Picker::new(
        editor.redraw().clone(),
        [
            PickerColumn::new("kind", |file: &StashFile, data: &StashFiles| {
                let scope = if file.untracked() {
                    "diff.plus"
                } else {
                    "ui.text.inactive"
                };
                Span::styled(file.layer().label(), data.theme.get(scope)).into()
            }),
            PickerColumn::new("path", |file: &StashFile, _| {
                file.path().to_string_lossy().into_owned().into()
            }),
        ],
        1,
        [],
        data,
        move |cx, file, action| {
            let request = view::features::change_review::HistoricalReviewRequest::stash(
                cx.editor,
                repo.clone(),
                entry.clone(),
                file.clone(),
            );
            cx.editor.open_historical_review(request, action);
        },
    )
    .with_title("Stash files · Enter to review")
    .with_file_updates(
        editor,
        |data| data.repo.root(),
        |_, _, data, cancel| {
            Box::pin(async move {
                event::cancelable_future(data.repo.stash_files(&data.entry), cancel)
                    .await
                    .unwrap_or_else(|| Err(anyhow::anyhow!("Stash browser closed")))
            })
        },
    )
    .refresh_on_open()
}

struct StashFiles {
    repo: GitRepository,
    entry: StashEntry,
    theme: Arc<theme::Theme>,
}

#[cfg(all(test, feature = "integration"))]
mod tests;
