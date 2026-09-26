//! Change navigation, changed-file pickers, and change-reset commands.

use crate::{
    commands::{context::Context, navigation::push_jump},
    ui::{overlay::overlaid, Picker, PickerColumn},
};
use ::vcs::{FileChange, Hunk};
use editor_core::{movement::Direction, Range, RopeSlice, Selection};
use event::status;
use std::{error::Error, path::Path};
use tui::{text::Span, widgets::Cell};
use view::{document::Mode, icons::ICONS, theme::Style, Editor};

pub(super) fn changed_file_picker(cx: &mut Context) {
    changed_file_picker_for_scope(
        cx,
        vcs::ChangedFileScope::Directory(loader::find_workspace().0),
    );
}

pub(super) fn changed_file_picker_in_repository(cx: &mut Context) {
    changed_file_picker_for_scope(
        cx,
        vcs::ChangedFileScope::Repository(stdx::env::current_working_dir()),
    );
}

fn changed_file_picker_for_scope(cx: &mut Context, scope: vcs::ChangedFileScope) {
    struct ChangedFileEntry {
        change: FileChange,
        display_path: String,
    }

    pub struct FileChangeData {
        icons: bool,
        style_untracked: Style,
        style_modified: Style,
        style_conflict: Style,
        style_deleted: Style,
        style_renamed: Style,
    }

    fn display_path(change: &FileChange, worktree_root: &Path) -> String {
        let display_path = |path: &Path| {
            stdx::path::get_relative_path_from(path, worktree_root)
                .display()
                .to_string()
        };

        match change {
            FileChange::Untracked { path }
            | FileChange::Modified { path }
            | FileChange::Conflict { path }
            | FileChange::Deleted { path } => display_path(path),
            FileChange::Renamed { from_path, to_path } => {
                format!("{} -> {}", display_path(from_path), display_path(to_path))
            }
        }
    }

    fn change_column<'a>(entry: &'a ChangedFileEntry, data: &FileChangeData) -> Cell<'a> {
        let icons = ICONS.load();
        let (plain, icon, label, style) = match &entry.change {
            FileChange::Untracked { .. } => (
                "+ untracked",
                icons.vcs().added(),
                "untracked",
                data.style_untracked,
            ),
            FileChange::Modified { .. } => (
                "~ modified",
                icons.vcs().modified(),
                "modified",
                data.style_modified,
            ),
            FileChange::Conflict { .. } => (
                "x conflict",
                icons.vcs().conflict(),
                "conflict",
                data.style_conflict,
            ),
            FileChange::Deleted { .. } => (
                "- deleted",
                icons.vcs().removed(),
                "deleted",
                data.style_deleted,
            ),
            FileChange::Renamed { .. } => (
                "> renamed",
                icons.vcs().renamed(),
                "renamed",
                data.style_renamed,
            ),
        };
        let content = if data.icons {
            icon.map_or_else(|| label.to_string(), |icon| format!("{icon}{label}"))
        } else {
            plain.to_string()
        };
        Span::styled(content, style).into()
    }

    fn path_column<'a>(entry: &'a ChangedFileEntry, _data: &FileChangeData) -> Cell<'a> {
        entry.display_path.as_str().into()
    }

    if !scope.path().exists() {
        cx.editor.set_error(|| "Changed file scope does not exist");
        return;
    }

    let workspace_root = loader::find_workspace_in(scope.path()).0;
    let display_root = match &scope {
        vcs::ChangedFileScope::Directory(path) => Some(path.clone()),
        vcs::ChangedFileScope::Repository(_) => None,
    };

    let added = cx.editor.theme.get("diff.plus");
    let modified = cx.editor.theme.get("diff.delta");
    let conflict = cx.editor.theme.get("diff.delta.conflict");
    let deleted = cx.editor.theme.get("diff.minus");
    let renamed = cx.editor.theme.get("diff.delta.moved");

    let columns = [
        PickerColumn::new("change", change_column),
        PickerColumn::new("path", path_column),
    ];

    let picker = Picker::new(
        columns,
        1, // path
        [],
        FileChangeData {
            icons: cx.editor.config().icons,
            style_untracked: added,
            style_modified: modified,
            style_conflict: conflict,
            style_deleted: deleted,
            style_renamed: renamed,
        },
        |cx, entry: &ChangedFileEntry, action| {
            let path_to_open = entry.change.path();
            if let Err(err) = cx.editor.open(path_to_open, action) {
                cx.editor.set_error(|| {
                    if let Some(err) = err.source() {
                        format!("{}", err)
                    } else {
                        format!("unable to open \"{}\"", path_to_open.display())
                    }
                });
            }
        },
    )
    .with_preview(|_editor, entry| Some((entry.change.path().into(), None)));
    let injector = picker.injector();

    let trust_full = cx
        .editor
        .workspace_trust
        .query(&workspace_root, loader::workspace_trust::TrustQuery::Git)
        .is_trusted();
    cx.editor.diff_providers.clone().for_each_changed_file(
        scope,
        trust_full,
        move |worktree_root, change| match change {
            Ok(change) => injector
                .push(ChangedFileEntry {
                    display_path: display_path(
                        &change,
                        display_root.as_deref().unwrap_or(worktree_root),
                    ),
                    change,
                })
                .is_ok(),
            Err(err) => {
                status::report_blocking(err);
                true
            }
        },
    );
    cx.push_layer(Box::new(overlaid(picker)));
}

pub(super) fn goto_first_change(cx: &mut Context) {
    goto_first_change_impl(cx, false);
}

pub(super) fn goto_last_change(cx: &mut Context) {
    goto_first_change_impl(cx, true);
}

fn goto_first_change_impl(cx: &mut Context, reverse: bool) {
    let editor = &mut cx.editor;
    let (view, doc) = current!(editor);
    if let Some(handle) = doc.diff_handle() {
        let hunk = {
            let diff = handle.load();
            let idx = if reverse {
                diff.len().saturating_sub(1)
            } else {
                0
            };
            diff.nth_hunk(idx)
        };
        if hunk != Hunk::NONE {
            let range = hunk_range(hunk, doc.text().slice(..));
            push_jump(view, doc);
            doc.set_selection(view.id, Selection::single(range.anchor, range.head));
        }
    }
}

pub(super) fn goto_next_change(cx: &mut Context) {
    goto_next_change_impl(cx, Direction::Forward)
}

pub(super) fn goto_prev_change(cx: &mut Context) {
    goto_next_change_impl(cx, Direction::Backward)
}

fn goto_next_change_impl(cx: &mut Context, direction: Direction) {
    let count = cx.count() as u32 - 1;
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);
        let doc_text = doc.text().slice(..);
        let diff_handle = if let Some(diff_handle) = doc.diff_handle() {
            diff_handle
        } else {
            editor.set_status("Diff is not available in current buffer");
            return;
        };

        let selection = doc.selection(view.id).clone().transform(|range| {
            let cursor_line = range.cursor_line(doc_text) as u32;

            let diff = diff_handle.load();
            let hunk_idx = match direction {
                Direction::Forward => diff
                    .next_hunk(cursor_line)
                    .map(|idx| (idx + count).min(diff.len() - 1)),
                Direction::Backward => diff
                    .prev_hunk(cursor_line)
                    .map(|idx| idx.saturating_sub(count)),
            };
            let Some(hunk_idx) = hunk_idx else {
                return range;
            };
            let hunk = diff.nth_hunk(hunk_idx);
            let new_range = hunk_range(hunk, doc_text);
            if editor.mode == Mode::Select {
                let head = if new_range.head < range.anchor {
                    new_range.anchor
                } else {
                    new_range.head
                };

                Range::new(range.anchor, head)
            } else {
                new_range.with_direction(direction)
            }
        });

        push_jump(view, doc);
        doc.set_selection(view.id, selection)
    };
    cx.editor.apply_motion(motion);
}

/// Returns the [Range] for a [Hunk] in the given text.
/// Additions and modifications cover the added and modified ranges.
/// Deletions are represented as the point at the start of the deletion hunk.
pub(super) fn hunk_range(hunk: Hunk, text: RopeSlice) -> Range {
    let anchor = text.line_to_char(hunk.after.start as usize);
    let head = if hunk.after.is_empty() {
        anchor + 1
    } else {
        text.line_to_char(hunk.after.end as usize)
    };

    Range::new(anchor, head)
}

pub(super) mod typed {
    //! Typable vcs commands.

    use crate::{compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::bail;
    use editor_core::{Tendril, Transaction};

    #[cold]
    pub(in crate::commands) fn reset_diff_change(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let editor = &mut cx.editor;
        let scrolloff = editor.config().scrolloff;

        let (view, doc) = current!(editor);
        let Some(handle) = doc.diff_handle() else {
            bail!("Diff is not available in the current buffer")
        };

        let diff = handle.load();
        let doc_text = doc.text().slice(..);
        let diff_base = diff.diff_base();
        let mut changes = 0;

        let transaction = Transaction::change(
            doc.text(),
            diff.hunks_intersecting_line_ranges(doc.selection(view.id).line_ranges(doc_text))
                .map(|hunk| {
                    changes += 1;
                    let start = diff_base.line_to_char(hunk.before.start as usize);
                    let end = diff_base.line_to_char(hunk.before.end as usize);
                    let text: Tendril = diff_base.slice(start..end).chunks().collect();
                    (
                        doc_text.line_to_char(hunk.after.start as usize),
                        doc_text.line_to_char(hunk.after.end as usize),
                        (!text.is_empty()).then_some(text),
                    )
                }),
        );
        if changes == 0 {
            bail!("There are no changes under any selection");
        }

        drop(diff); // make borrow check happy
        doc.apply(&transaction, view.id);
        doc.append_changes_to_history(view);
        view.ensure_cursor_in_view(doc, scrolloff);
        cx.editor.set_status(format!(
            "Reset {changes} change{}",
            if changes == 1 { "" } else { "s" }
        ));
        Ok(())
    }
}
