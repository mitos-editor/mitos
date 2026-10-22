//! Quicklist capture, traversal, and picker commands.

use crate::{
    commands::{context::Context, picker::PathStyleConfig},
    keymap::component::PickerItemAction,
    ui::{self, overlay::overlaid, picker::PickerCallbackResult, Picker},
};
use editor_core::movement::Direction;
use std::{borrow::Cow, path::PathBuf};
use view::{
    document::SCRATCH_BUFFER_NAME,
    quicklist::{QuicklistEntry, QuicklistTarget},
    Editor,
};

fn quicklist_entry_line_range(editor: &Editor, entry: &QuicklistEntry) -> Option<(usize, usize)> {
    let text = match &entry.target {
        QuicklistTarget::Path(path) => editor
            .document_by_path(path)
            .map(|doc| doc.text().slice(..)),
        QuicklistTarget::Document(id) => editor.documents.get(id).map(|doc| doc.text().slice(..)),
    };

    entry.position.line_range(text)
}

#[derive(Clone, PartialEq)]
struct QuicklistMeta {
    index: usize,
    entry: QuicklistEntry,
    path: Option<PathBuf>,
    label: String,
    line: Option<usize>,
    is_current: bool,
}

pub(super) fn add_to_quicklist(cx: &mut Context) {
    cx.editor.add_to_quicklist(cx.editor.tree.focus());
    cx.editor.set_status("Current location added to quicklist");
}

fn quicklist_items(editor: &Editor) -> Vec<QuicklistMeta> {
    editor
        .quicklist
        .entries()
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, entry)| {
            let path = match &entry.target {
                QuicklistTarget::Path(path) => {
                    Some(stdx::path::get_relative_path(path).into_owned())
                }
                QuicklistTarget::Document(id) => editor
                    .documents
                    .get(id)
                    .and_then(|doc| doc.path())
                    .map(stdx::path::get_relative_path)
                    .map(Cow::into_owned),
            };
            let label = path
                .as_deref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|| match &entry.target {
                    QuicklistTarget::Document(id) => format!("{SCRATCH_BUFFER_NAME} ({id})"),
                    QuicklistTarget::Path(_) => unreachable!(),
                });

            QuicklistMeta {
                index,
                path,
                label,
                line: quicklist_entry_line_range(editor, &entry).map(|(start, _)| start + 1),
                is_current: editor.quicklist.current() == Some(index),
                entry,
            }
        })
        .collect()
}

fn refreshed_quicklist(editor: &Editor) -> PickerCallbackResult<QuicklistMeta, PathStyleConfig> {
    PickerCallbackResult::Refresh {
        options: quicklist_items(editor),
        editor_data: PathStyleConfig::new(editor),
    }
}

pub(super) fn quicklist_picker(cx: &mut Context) {
    if cx.editor.quicklist.entries().is_empty() {
        cx.editor.set_error(|| "No quicklist entries available");
        return;
    }

    let columns = [
        ui::PickerColumn::new("path", |item: &QuicklistMeta, config: &PathStyleConfig| {
            item.path.as_deref().map_or_else(
                || item.label.as_str().into(),
                |path| config.stylize(Some(path), None),
            )
        })
        .with_prefix(|item, config| config.icon(item.path.as_deref())),
        ui::PickerColumn::new("line", |item: &QuicklistMeta, _| {
            item.line
                .map_or_else(String::new, |line| line.to_string())
                .into()
        }),
        ui::PickerColumn::new("flags", |item: &QuicklistMeta, _| {
            if item.is_current {
                " (*)".into()
            } else {
                "".into()
            }
        }),
    ];

    let initial_cursor = cx.editor.quicklist.current().unwrap_or(0) as u32;
    let mut revision = cx.editor.quicklist.revision();

    let picker = Picker::new_with_callback_result(
        cx.editor.redraw().clone(),
        columns,
        0,
        quicklist_items(cx.editor),
        PathStyleConfig::new(cx.editor),
        |cx, meta, action| {
            // A resumed picker or a replaced quicklist may still have an old row.
            if cx.editor.quicklist.entries().get(meta.index) != Some(&meta.entry) {
                return refreshed_quicklist(cx.editor);
            }
            let view_id = cx.editor.tree.focus();
            if cx
                .editor
                .activate_quicklist_entry(view_id, &meta.entry, action)
            {
                cx.editor.quicklist.set_current(Some(meta.index));
            }
            PickerCallbackResult::Close
        },
    )
    .with_title("Quicklist")
    .with_editor_updates(move |cx| {
        let current = cx.editor.quicklist.revision();
        if !cx.resumed && current == revision {
            return None;
        }
        revision = current;
        Some(quicklist_items(cx.editor))
    })
    .with_item_actions(
        |maps| &maps.quicklist_picker,
        |cx, meta, action| {
            if action == PickerItemAction::RemoveQuicklistEntry
                && cx.editor.quicklist.entries().get(meta.index) == Some(&meta.entry)
            {
                cx.editor.quicklist.remove(meta.index);
            }
            refreshed_quicklist(cx.editor)
        },
    )
    .with_initial_cursor(initial_cursor)
    .with_quicklist(|_, meta| Some(meta.entry.clone()))
    .with_preview(|editor, meta| {
        let path_or_id = match &meta.entry.target {
            QuicklistTarget::Path(path) => path.as_path().into(),
            QuicklistTarget::Document(id) => (*id).into(),
        };
        Some((path_or_id, quicklist_entry_line_range(editor, &meta.entry)))
    });

    cx.push_layer(Box::new(overlaid(picker)));
}

pub(super) fn goto_next_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Forward, false);
}

pub(super) fn goto_prev_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Backward, false);
}

pub(super) fn goto_next_file_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Forward, true);
}

pub(super) fn goto_prev_file_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Backward, true);
}

fn goto_quicklist_impl(cx: &mut Context, direction: Direction, same_file: bool) {
    let view_id = cx.editor.tree.focus();
    let jumped = match direction {
        Direction::Forward => cx
            .editor
            .jump_next_quicklist(view_id, cx.count(), same_file),
        Direction::Backward => cx
            .editor
            .jump_prev_quicklist(view_id, cx.count(), same_file),
    };

    if !jumped {
        let message = if same_file {
            "No quicklist entries available in the current file"
        } else {
            "No quicklist entries available"
        };
        cx.editor.set_error(|| message);
    }
}
