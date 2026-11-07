//! File explorer views and create, move, copy, delete, and yank actions.
//!
//! Every change goes through [`Editor::request_file_operation`], so open buffers follow
//! moves, language servers get their will/did notifications, and existing paths are
//! never overwritten. The explorer refreshes from the editor's file-operation revision,
//! so it updates even when file watching is disabled.

use std::{
    borrow::Cow,
    error::Error,
    path::{Path, PathBuf, MAIN_SEPARATOR},
    sync::Arc,
};

use super::{icon_span, FileExplorer, Picker, PickerColumn};
use editor_core::unicode::segmentation::UnicodeSegmentation;
use theme::{icons::ICONS, Theme};
use tui::text::Span;
use view::{
    editor::FileOperation,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Editor,
};

use crate::{
    compositor::{self, Context},
    keymap::component::PickerItemAction,
    ui::{
        self,
        picker::{self, PickerCallbackResult},
        prompt::Movement,
        Prompt, PromptEvent,
    },
};

mod tree;

use tree::{matches, query};
pub(super) use tree::{search_index, ExplorerTree};

/// (path, is the path a directory?)
pub(super) type ExplorerItem = (PathBuf, bool);
#[derive(Clone)]
pub struct ExplorerData {
    pub(super) root: PathBuf,
    pub(super) theme: Arc<Theme>,
    pub(super) show_icons: bool,
    pub(super) tree: Option<ExplorerTree>,
}

pub(super) fn picker(
    root: PathBuf,
    editor: &Editor,
    tree: bool,
) -> Result<FileExplorer, std::io::Error> {
    let tree_mode = tree;
    let tree = tree.then(ExplorerTree::default);
    let options = content(&root, tree.as_ref(), &editor.config().file_explorer, || {
        false
    })?;

    let columns = [PickerColumn::new(
        "path",
        |(path, is_dir): &ExplorerItem, data: &ExplorerData| {
            let ExplorerData {
                root,
                theme,
                show_icons,
                tree,
            } = data;
            let name = path.file_name();
            let name = name.map_or_else(|| Cow::Borrowed(".."), |name| name.to_string_lossy());
            let directory_style = theme.get("ui.text.directory");

            let prefix = tree
                .as_ref()
                .filter(|_| !show_icons)
                .map_or_else(String::new, |tree| {
                    let depth = path
                        .strip_prefix(root)
                        .map_or(0, |path| path.components().count().saturating_sub(1));
                    let marker = if *is_dir {
                        if tree.is_expanded(path) {
                            "▾ "
                        } else {
                            "▸ "
                        }
                    } else {
                        "  "
                    };
                    format!("{}{marker}", "  ".repeat(depth))
                });
            if *is_dir {
                Span::styled(format!("{prefix}{name}/"), directory_style).into()
            } else {
                format!("{prefix}{name}").into()
            }
        },
    )
    .with_prefix(|(path, is_dir), data| {
        let ExplorerData {
            root,
            theme,
            show_icons,
            tree,
        } = data;
        if !show_icons {
            return None;
        }
        let icon = if *is_dir {
            let name = path.file_name();
            let is_open =
                name.is_none() || tree.as_ref().is_some_and(|tree| tree.is_expanded(path));
            let name = name.map_or_else(|| Cow::Borrowed(".."), |name| name.to_string_lossy());
            ICONS.fs().directory().get_with_style_or_default(
                &name,
                is_open,
                theme,
                theme.get("ui.text.directory"),
            )
        } else {
            ICONS.fs().file().get_with_style_or_default(path, theme)
        };
        let mut span = icon_span(icon.with_no_left_padding());
        if tree.is_some() {
            let depth = path
                .strip_prefix(root)
                .map_or(0, |path| path.components().count().saturating_sub(1));
            span.content = format!("{}{}", "  ".repeat(depth), span.content).into();
        }
        Some(span)
    })];
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        columns,
        0,
        options,
        ExplorerData {
            root,
            theme: Arc::new(editor.theme.clone()),
            show_icons: editor.config().icons,
            tree,
        },
        |_, _, _| picker::PickerCallbackResult::KeepOpen,
    )
    .with_data_callback(move |cx, (path, is_dir): &(PathBuf, bool), data, action| {
        if *is_dir {
            let mut tree = data.tree.clone();
            let new_root = if let Some(tree) = &mut tree {
                tree.searching = false;
                tree.index = Default::default();
                tree.reveal(&data.root, path);
                tree.expanded.toggle(path.clone());
                data.root.clone()
            } else {
                stdx::path::normalize(path)
            };
            return match content(
                &new_root,
                tree.as_ref(),
                &cx.editor.config().file_explorer,
                || false,
            ) {
                Ok(options) => picker::PickerCallbackResult::Replace {
                    options,
                    editor_data: ExplorerData {
                        root: new_root,
                        theme: Arc::new(cx.editor.theme.clone()),
                        show_icons: cx.editor.config().icons,
                        tree,
                    },
                },
                Err(err) => {
                    cx.editor.set_error(|| err.to_string());
                    picker::PickerCallbackResult::KeepOpen
                }
            };
        }

        if let Err(err) = cx.editor.open(path, action) {
            cx.editor.set_error(|| {
                if let Some(err) = err.source() {
                    format!("{}", err)
                } else {
                    format!("unable to open \"{}\"", path.display())
                }
            });
            return picker::PickerCallbackResult::KeepOpen;
        }
        picker::PickerCallbackResult::Close
    })
    .with_title("File Explorer")
    .with_preview(|_editor, (path, _is_dir)| Some((path.as_path().into(), None)))
    .with_quicklist(|_editor, (path, is_dir)| {
        if *is_dir {
            return None;
        }

        Some(QuicklistEntry {
            target: QuicklistTarget::Path(path.clone()),
            position: QuicklistPosition::None,
        })
    })
    .with_file_updates(
        editor,
        |data| &data.root,
        |query, editor, data, cancel| {
            let config = editor.config().file_explorer.clone();
            let query = query.to_owned();
            Box::pin(async move {
                Ok(tokio::task::spawn_blocking(move || {
                    if let Some(tree) = data.tree.as_ref().filter(|_| !query.is_empty()) {
                        let rows = search_index(&data.root, &config, || cancel.is_canceled())?;
                        let mut index = tree.index.lock().unwrap();
                        if cancel.is_canceled() {
                            return Ok(Vec::new());
                        }
                        *index = Some(rows.clone());
                        return Ok(matches(&data.root, &query, &rows).0);
                    }
                    content(&data.root, data.tree.as_ref(), &config, || {
                        cancel.is_canceled()
                    })
                })
                .await??)
            })
        },
    )
    .with_refresh_generation(editor, Editor::file_operation_revision)
    .with_item_key(|(path, _)| path.clone())
    .with_item_data_actions(|maps| &maps.file_explorer, action);

    Ok(if tree_mode {
        picker.with_query_options(query)
    } else {
        picker
    })
}

fn content(
    root: &Path,
    tree: Option<&ExplorerTree>,
    config: &editor_config::FileExplorerConfig,
    cancelled: impl Fn() -> bool,
) -> std::io::Result<Vec<ExplorerItem>> {
    let Some(tree) = tree else {
        return directory_content_cancellable(root, config, cancelled);
    };
    let children = |path: &Path| {
        directory_content_cancellable(path, config, &cancelled).map(|rows| {
            rows.into_iter()
                .filter(|(path, _)| path.file_name().is_some())
                .collect()
        })
    };
    tree.expanded
        .rows(
            children(root)?,
            |(path, _)| path,
            |(path, is_dir)| {
                if *is_dir && !cancelled() {
                    children(path)
                } else {
                    Ok(Vec::new())
                }
            },
        )
        .map(|rows| rows.into_iter().map(|(item, _)| item).collect())
}

pub(super) fn directory_content(
    root: &Path,
    config: &editor_config::FileExplorerConfig,
) -> Result<Vec<(PathBuf, bool)>, std::io::Error> {
    directory_content_cancellable(root, config, || false)
}

fn directory_content_cancellable(
    root: &Path,
    config: &editor_config::FileExplorerConfig,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<(PathBuf, bool)>, std::io::Error> {
    // Keep the previous listing when its directory disappears or becomes unreadable.
    std::fs::read_dir(root)?;
    let mut content: Vec<(PathBuf, bool)> = crate::files::explorer_options(config)
        .walker(root)
        .types(crate::files::excluded_types())
        .build()
        .take_while(|_| !cancelled())
        .filter_map(|entry| {
            entry
                .map(|entry| {
                    let path = entry.path();
                    let is_dir = search::entry_file_type(&entry).is_some_and(|kind| kind.is_dir());
                    (path.to_path_buf(), is_dir)
                })
                .ok()
                .filter(|entry| entry.0 != root)
        })
        .collect();

    content.sort_by(|(path1, is_dir1), (path2, is_dir2)| (!is_dir1, path1).cmp(&(!is_dir2, path2)));

    if root.parent().is_some() {
        content.insert(0, (root.join(".."), true));
    }

    Ok(content)
}

type ActionResult = PickerCallbackResult<ExplorerItem, ExplorerData>;

/// Run an explorer item action for the selected entry of the directory `data.root`.
pub(super) fn action(
    cx: &mut Context,
    item: Option<&ExplorerItem>,
    data: &ExplorerData,
    action: PickerItemAction,
) -> ActionResult {
    // `../` is a navigation entry, not something to move, copy or delete.
    let selected = item.filter(|(path, _)| path.file_name().is_some());
    match action {
        PickerItemAction::CreatePath => prompt(
            "create:",
            directory_prefix(if data.tree.is_some() {
                selected.map_or(data.root.as_path(), |(path, is_dir)| {
                    if *is_dir {
                        path.as_path()
                    } else {
                        path.parent().unwrap_or(&data.root)
                    }
                })
            } else {
                &data.root
            }),
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
                yank(cx, &stdx::path::normalize(path));
            }
            PickerCallbackResult::KeepOpen
        }
        _ => PickerCallbackResult::KeepOpen,
    }
}

fn nothing_selected(cx: &mut Context) -> ActionResult {
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
    stdx::path::canonicalize(input)
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
    path.extension().map(|extension| {
        Movement::BackwardChar(extension.to_string_lossy().graphemes(false).count() + 1)
    })
}

fn yank(cx: &mut Context, path: &Path) {
    let register = cx
        .interaction
        .selected_register
        .take()
        .unwrap_or(cx.editor.config().clipboard.default_yank_register);
    let editor = &mut cx.editor;
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
) -> ActionResult {
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
) -> ActionResult {
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
