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
        Command::Stashes(path) => open_stashes(cx, path, workspace),
        Command::Run(action @ GitAction::File(..)) | Command::Confirmed(action) => {
            start(cx, workspace, action)
        }
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
                GitAction::File(..) => unreachable!(),
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
                let task = match result.and_then(|prepared| prepared.admit(editor)) {
                    Ok(task) => task,
                    Err(error) => {
                        editor.set_error(|| error.to_string());
                        return None;
                    }
                };
                Some(
                    Job::with_callback(async move {
                        let completion = task.execute().await;
                        Ok(Callback::Editor(Box::new(move |editor| {
                            completion.finish(editor)
                        })))
                    })
                    .wait_before_exiting(),
                )
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

pub(super) fn help(
    keys: &ui_core::keymap::Keymap<crate::keymap::component::PickerAction>,
) -> String {
    use crate::keymap::component::{action_hint, PickerAction};
    format!(
        " {} actions ",
        action_hint(keys, &PickerAction::Item(PickerItemAction::Actions))
    )
}

pub(super) fn generation(editor: &Editor) -> u64 {
    editor.handlers().git_operations.revision()
}

pub(super) fn open_stashes(cx: &mut Context, path: PathBuf, workspace: PathBuf) {
    request_stashes(cx.editor, cx.jobs, path, workspace);
}

pub(super) fn request_stashes(
    editor: &mut Editor,
    jobs: &mut crate::job::Jobs,
    path: PathBuf,
    workspace: PathBuf,
) {
    editor.set_status("Loading stashes… (newest 500)");
    jobs.callback(async move {
        let result = async {
            let repo =
                tokio::task::spawn_blocking(move || GitRepository::discover(&path)).await??;
            let entries = repo.stashes().await?;
            Ok::<_, anyhow::Error>((repo, entries))
        }
        .await;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| match result {
                Ok((repo, entries)) => {
                    if entries.is_empty() {
                        editor.set_status("No stashes in this repository");
                    }
                    compositor.push(Box::new(overlaid(stash_picker(
                        editor, repo, workspace, entries,
                    ))));
                }
                Err(error) => editor.set_error(|| error.to_string()),
            },
        )))
    });
}

struct StashData {
    repo: GitRepository,
}
fn stash_picker(
    editor: &Editor,
    repo: GitRepository,
    workspace: PathBuf,
    entries: Vec<StashEntry>,
) -> Picker<StashEntry, StashData> {
    let inspect_repo = repo.clone();
    let inspect_workspace = workspace.clone();
    let action_repo = repo.clone();
    let action_workspace = workspace.clone();
    Picker::new(
        editor.redraw().clone(),
        [
            PickerColumn::new("stash", |entry: &StashEntry, _| {
                entry.selector.as_str().into()
            }),
            PickerColumn::new("commit", |entry: &StashEntry, _| {
                entry.id()[..12].to_owned().into()
            }),
            PickerColumn::new("message", |entry: &StashEntry, _| {
                entry.message.as_str().into()
            }),
        ],
        2,
        entries,
        StashData { repo },
        move |cx, entry, _| {
            open_stash_files(
                cx,
                inspect_repo.clone(),
                inspect_workspace.clone(),
                entry.clone(),
            );
        },
    )
    .with_help(|maps| help(&maps.stash_picker))
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

fn open_stash_files(cx: &mut Context, repo: GitRepository, workspace: PathBuf, entry: StashEntry) {
    cx.editor.set_status("Loading stash files…");
    cx.jobs.callback(async move {
        let files = repo.stash_files(&entry).await;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| match files {
                Ok(files) => {
                    let review_repo = repo.clone();
                    let review_workspace = workspace.clone();
                    let review_entry = entry.clone();
                    let picker = Picker::new(
                        editor.redraw().clone(),
                        [
                            PickerColumn::new("kind", |file: &StashFile, _: &()| {
                                file.layer().label().into()
                            }),
                            PickerColumn::new("path", |file: &StashFile, _| {
                                file.path().to_string_lossy().into_owned().into()
                            }),
                        ],
                        1,
                        files,
                        (),
                        move |cx, file, _| {
                            let request =
                                view::features::change_review::HistoricalReviewRequest::capture(
                                    cx.editor,
                                    &review_repo.root().join(file.path()),
                                );
                            let trusted = cx
                                .editor
                                .workspace_trust
                                .query(&review_workspace, workspace::trust::TrustQuery::Git)
                                .is_trusted();
                            let repo = review_repo.clone();
                            let entry = review_entry.clone();
                            let file = file.clone();
                            let title = format!(
                                "{} ({}) · {} · HEAD → {}",
                                entry.selector,
                                &entry.id()[..12],
                                file.path().display(),
                                file.layer().label()
                            );
                            cx.jobs.callback(async move {
                                let result = request
                                    .run(move || repo.read_stash_file(&entry, &file, trusted))
                                    .await;
                                Ok(Callback::EditorCompositor(Box::new(
                                    move |editor, compositor| match result {
                                        Ok((source, review)) => compositor.push(Box::new(
                                            ui::Popup::new(
                                                ui::change_review::ReviewView::ID,
                                                ui::change_review::ReviewView::from_shared(
                                                    title, review,
                                                )
                                                .with_source(source),
                                            )
                                            .auto_close(true),
                                        )),
                                        Err(error) => editor.set_error(|| error.to_string()),
                                    },
                                )))
                            });
                        },
                    );
                    compositor.push(Box::new(overlaid(picker)));
                    editor.set_status(format!(
                        "{} ({}) · {} · Enter to review a saved file",
                        entry.selector,
                        &entry.id()[..12],
                        entry.message
                    ));
                }
                Err(error) => editor.set_error(|| error.to_string()),
            },
        )))
    });
}

#[cfg(all(test, feature = "integration"))]
mod tests;
