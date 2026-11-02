//! File explorer actions: create, move, copy, delete and yank paths without leaving it.
//!
//! Every change goes through [`Editor::request_file_operation`], so open buffers follow
//! moves, language servers get their will/did notifications, and existing paths are
//! never overwritten. The explorer refreshes from the editor's file-operation revision,
//! so it updates even when file watching is disabled.

use std::{
    path::{Path, PathBuf, MAIN_SEPARATOR},
    sync::Arc,
};

use theme::Theme;
use view::{editor::FileOperation, Editor};

use crate::{
    compositor::{self, Context},
    keymap::component::PickerItemAction,
    ui::{self, picker::PickerCallbackResult, prompt::Movement, Prompt, PromptEvent},
};

/// (path, is the path a directory?)
pub(super) type ExplorerItem = (PathBuf, bool);
/// (directory shown, theme, show icons)
pub(super) type ExplorerData = (PathBuf, Arc<Theme>, bool);

type Result = PickerCallbackResult<ExplorerItem, ExplorerData>;

/// Run an explorer item action for the selected entry of the directory `data.0`.
pub(super) fn action(
    cx: &mut Context,
    item: Option<&ExplorerItem>,
    data: &ExplorerData,
    action: PickerItemAction,
) -> Result {
    // `../` is a navigation entry, not something to move, copy or delete.
    let selected = item.filter(|(path, _)| path.file_name().is_some());
    match action {
        PickerItemAction::CreatePath => prompt(
            "create:",
            directory_prefix(&data.0),
            None,
            |editor, input| {
                let is_dir = input.ends_with(MAIN_SEPARATOR) || input.ends_with('/');
                editor.request_file_operation(FileOperation::Create {
                    path: input_path(input),
                    is_dir,
                })
            },
        ),
        PickerItemAction::MovePath | PickerItemAction::CopyPath => {
            let Some((path, is_dir)) = selected else {
                return nothing_selected(cx);
            };
            let copy = action == PickerItemAction::CopyPath;
            if copy && *is_dir {
                cx.editor
                    .set_error(|| "Copying directories is not supported");
                return PickerCallbackResult::KeepOpen;
            }
            let from = path.clone();
            prompt(
                if copy { "copy to:" } else { "move to:" },
                display(path),
                before_extension(path, *is_dir),
                move |editor, input| {
                    let to = into_directory(&from, input_path(input));
                    let from = from.clone();
                    editor.request_file_operation(if copy {
                        FileOperation::Copy { from, to }
                    } else {
                        FileOperation::Move { from, to }
                    })
                },
            )
        }
        PickerItemAction::DeletePath => {
            let Some((path, is_dir)) = selected else {
                return nothing_selected(cx);
            };
            let (path, recursive) = (path.clone(), *is_dir);
            let label = if recursive {
                format!("delete {}/ and its contents? (y/n):", display(&path))
            } else {
                format!("delete {}? (y/n):", display(&path))
            };
            confirm(label, move |editor| {
                editor.request_file_operation(FileOperation::Delete {
                    path: path.clone(),
                    recursive,
                })
            })
        }
        PickerItemAction::YankPath => {
            if let Some((path, _)) = item {
                yank(cx.editor, &stdx::path::normalize(path));
            }
            PickerCallbackResult::KeepOpen
        }
        _ => PickerCallbackResult::KeepOpen,
    }
}

fn nothing_selected(cx: &mut Context) -> Result {
    cx.editor.set_error(|| "Select a file or directory first");
    PickerCallbackResult::KeepOpen
}

/// Show a path as the user would type it: relative to the working directory, or with `~`.
fn display(path: &Path) -> String {
    stdx::path::get_relative_path(path)
        .to_string_lossy()
        .into_owned()
}

/// The prompt text for a new entry in `directory`, ending in a separator.
fn directory_prefix(directory: &Path) -> String {
    let directory = display(directory);
    if directory.is_empty() {
        directory
    } else {
        format!("{directory}{MAIN_SEPARATOR}")
    }
}

fn input_path(input: &str) -> PathBuf {
    stdx::path::canonicalize(input.trim())
}

/// Moving or copying onto an existing directory puts the entry inside it, like `:move`.
fn into_directory(from: &Path, to: PathBuf) -> PathBuf {
    match from.file_name() {
        Some(name) if to.is_dir() && stdx::path::normalize(from) != to => to.join(name),
        _ => to,
    }
}

/// Put the cursor before a file's extension, which a rename usually keeps.
fn before_extension(path: &Path, is_dir: bool) -> Option<Movement> {
    if is_dir {
        return None;
    }
    path.extension()
        .map(|extension| Movement::BackwardChar(extension.len() + 1))
}

fn yank(editor: &mut Editor, path: &Path) {
    let register = editor
        .selected_register
        .take()
        .unwrap_or(editor.config().clipboard.default_yank_register);
    let path = display(path);
    match editor.registers.write(register, vec![path.clone()]) {
        Ok(()) => editor.set_status(format!("Yanked {path} to register {register}")),
        Err(error) => editor.set_error(|| error.to_string()),
    }
}

/// Ask for a path above the explorer, then run `operation` with the entered text.
fn prompt(
    label: &'static str,
    prefill: String,
    movement: Option<Movement>,
    operation: impl Fn(&mut Editor, &str) -> anyhow::Result<()> + 'static,
) -> Result {
    let unchanged = prefill.clone();
    PickerCallbackResult::KeepOpenWithCallback(Box::new(
        move |compositor: &mut compositor::Compositor, cx: &mut Context| {
            let mut prompt = Prompt::new(
                label.into(),
                None,
                ui::completers::filename,
                move |cx, input: &str, event| {
                    if event != PromptEvent::Validate
                        || input.trim().is_empty()
                        || input == unchanged
                    {
                        return;
                    }
                    if let Err(error) = operation(cx.editor, input) {
                        cx.editor.set_error(|| error.to_string());
                    }
                },
            )
            .with_line(prefill, cx.editor);
            if let Some(movement) = movement {
                prompt.move_cursor(movement);
            }
            compositor.push(Box::new(prompt));
        },
    ))
}

/// Run `operation` only when the user answers `y`.
fn confirm(
    label: String,
    operation: impl Fn(&mut Editor) -> anyhow::Result<()> + 'static,
) -> Result {
    PickerCallbackResult::KeepOpenWithCallback(Box::new(
        move |compositor: &mut compositor::Compositor, _: &mut Context| {
            let prompt = Prompt::new(
                label.into(),
                None,
                ui::completers::none,
                move |cx, input: &str, event| {
                    if event != PromptEvent::Validate || !input.trim().eq_ignore_ascii_case("y") {
                        return;
                    }
                    if let Err(error) = operation(cx.editor) {
                        cx.editor.set_error(|| error.to_string());
                    }
                },
            );
            compositor.push(Box::new(prompt));
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_entries_are_prefixed_with_the_shown_directory() {
        let cwd = stdx::env::current_working_dir();
        assert_eq!(directory_prefix(&cwd), "");
        assert_eq!(
            directory_prefix(&cwd.join("src")),
            format!("src{MAIN_SEPARATOR}")
        );
    }

    #[test]
    fn renames_place_the_cursor_before_a_file_extension() {
        assert!(matches!(
            before_extension(Path::new("a/b.rs"), false),
            Some(Movement::BackwardChar(3))
        ));
        assert!(before_extension(Path::new("a/Makefile"), false).is_none());
        assert!(before_extension(Path::new("a/dir.d"), true).is_none());
    }

    #[test]
    fn moving_onto_a_directory_moves_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.txt");
        let target = dir.path().join("sub");
        std::fs::create_dir(&target).unwrap();
        assert_eq!(into_directory(&from, target.clone()), target.join("a.txt"));
        let new = dir.path().join("b.txt");
        assert_eq!(into_directory(&from, new.clone()), new);
    }
}
