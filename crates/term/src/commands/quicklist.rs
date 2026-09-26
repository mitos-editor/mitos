//! Quicklist traversal and picker commands.

use crate::{
    commands::{context::Context, picker::PathStyleConfig},
    ui::{self, overlay::overlaid, Picker},
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

pub(super) fn quicklist_picker(cx: &mut Context) {
    #[derive(Clone)]
    struct QuicklistMeta {
        index: usize,
        entry: QuicklistEntry,
        path: Option<PathBuf>,
        label: String,
        line: Option<usize>,
        is_current: bool,
    }

    let entries = cx.editor.quicklist.entries();
    if entries.is_empty() {
        cx.editor.set_error(|| "No quicklist entries available");
        return;
    }

    let items = entries
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, entry)| {
            let path = match &entry.target {
                QuicklistTarget::Path(path) => {
                    Some(stdx::path::get_relative_path(path).into_owned())
                }
                QuicklistTarget::Document(id) => cx
                    .editor
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
                line: quicklist_entry_line_range(cx.editor, &entry).map(|(start, _)| start + 1),
                is_current: cx.editor.quicklist.current() == Some(index),
                entry,
            }
        })
        .collect::<Vec<_>>();

    let columns = [
        ui::PickerColumn::new("path", |item: &QuicklistMeta, config: &PathStyleConfig| {
            item.path.as_deref().map_or_else(
                || item.label.as_str().into(),
                |path| config.stylize(Some(path), None),
            )
        }),
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

    let picker = Picker::new(
        columns,
        0,
        items,
        PathStyleConfig::new(cx.editor),
        |cx, meta, action| {
            let view_id = cx.editor.tree.focus;
            if cx
                .editor
                .activate_quicklist_entry(view_id, &meta.entry, action)
            {
                cx.editor.quicklist.set_current(Some(meta.index));
            }
        },
    )
    .with_initial_cursor(initial_cursor)
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
    let view_id = cx.editor.tree.focus;
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
