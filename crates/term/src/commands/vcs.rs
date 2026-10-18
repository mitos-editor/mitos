//! Hunk review, change navigation, changed-file pickers, and buffer restoration.

use crate::{
    commands::{context::Context, navigation::push_jump},
    ui::{overlay::overlaid, Picker, PickerColumn},
};
use ::vcs::{FileChange, FileStatus, Hunk};
use editor_core::{movement::Direction, Range, RopeSlice, Selection};
use std::{error::Error, path::Path};
use theme::icons::ICONS;
use theme::Style;
use tui::{text::Span, widgets::Cell};
use view::{document::Mode, Editor};

pub(super) fn show_change(cx: &mut Context) {
    request_change(cx.editor, cx.jobs);
}

fn request_change(editor: &mut Editor, jobs: &mut crate::job::Jobs) {
    use view::features::change_review::HunkReviewRequest;
    let request = match HunkReviewRequest::capture(editor) {
        Ok(request) => std::sync::Arc::new(request),
        Err(error) => {
            editor.set_error(|| error.to_string());
            return;
        }
    };
    let worker = request.clone();
    jobs.callback(async move {
        let result = worker.run().await;
        Ok(crate::job::Callback::EditorCompositor(Box::new(
            move |editor, compositor| {
                if !request.is_current(editor) {
                    return;
                }
                match result {
                    Ok((source, review))
                        if review.additions != 0 || review.deletions != 0 || review.truncated =>
                    {
                        let view = crate::ui::change_review::ReviewView::new(
                            "Current change · HEAD → buffer".into(),
                            review,
                        )
                        .with_source(source);
                        compositor.replace_or_push(
                            crate::ui::change_review::ReviewView::ID,
                            crate::ui::Popup::new(crate::ui::change_review::ReviewView::ID, view)
                                .auto_close(true),
                        );
                    }
                    Ok(_) => editor.set_status("There is no change at the current line"),
                    Err(error) => editor.set_error(|| error.to_string()),
                }
            },
        )))
    });
}

pub(super) fn blame_line(cx: &mut Context) {
    view::features::blame::request_details(cx.editor);
}

pub(super) fn toggle_inline_blame(cx: &mut Context) {
    use crate::config::ConfigEvent;
    use editor_config::InlineBlameShow;

    let mut settings = cx.config.current.editor_settings();
    settings.editor.inline_blame.show = match settings.editor.inline_blame.show {
        InlineBlameShow::Never => cx
            .editor
            .last_inline_blame_show
            .unwrap_or(InlineBlameShow::CursorLine),
        show @ (InlineBlameShow::CursorLine | InlineBlameShow::AllLines) => {
            cx.editor.last_inline_blame_show = Some(show);
            InlineBlameShow::Never
        }
    };
    if let Err(err) = cx
        .config
        .updates
        .send(ConfigEvent::Update(Box::new(settings)))
    {
        cx.editor
            .set_error(|| format!("Could not toggle inline blame: {err}"));
    }
}

pub(super) fn changed_file_picker(cx: &mut Context) {
    changed_file_picker_for_scope(
        cx,
        vcs::ChangedFileScope::Directory(workspace::find_workspace().0),
    );
}

pub(super) fn changed_file_picker_in_repository(cx: &mut Context) {
    changed_file_picker_for_scope(
        cx,
        vcs::ChangedFileScope::Repository(stdx::env::current_working_dir()),
    );
}

#[derive(Clone, PartialEq, Eq)]
struct ChangedFileEntry {
    status: FileStatus,
    display_path: String,
}

fn display_path(change: &FileChange, worktree_root: &Path) -> String {
    let display_path = |path: &Path| {
        stdx::path::get_relative_path_from(path, worktree_root)
            .display()
            .to_string()
    };

    match change {
        FileChange::Untracked { path }
        | FileChange::Added { path }
        | FileChange::Modified { path }
        | FileChange::Conflict { path }
        | FileChange::Deleted { path } => display_path(path),
        FileChange::Renamed { from_path, to_path } => {
            format!("{} -> {}", display_path(from_path), display_path(to_path))
        }
    }
}

fn refresh_changed_files(
    providers: vcs::DiffProviderRegistry,
    scope: vcs::ChangedFileScope,
    trust_full: bool,
    display_root: Option<std::path::PathBuf>,
    cancel: event::TaskHandle,
) -> futures_util::future::BoxFuture<'static, anyhow::Result<Vec<ChangedFileEntry>>> {
    Box::pin(async move {
        let (tx, mut results) = tokio::sync::mpsc::unbounded_channel();
        providers.for_each_file_status(scope, trust_full, move |root, change| {
            if cancel.is_canceled() {
                return false;
            }
            tx.send(change.map(|change| ChangedFileEntry {
                display_path: display_path(
                    change.change(),
                    display_root.as_deref().unwrap_or(root),
                ),
                status: change,
            }))
            .is_ok()
        });
        let mut entries = Vec::new();
        while let Some(entry) = results.recv().await {
            entries.push(entry?);
        }
        entries.sort_by(|left, right| left.status.path().cmp(right.status.path()));
        Ok(entries)
    })
}

fn changed_file_picker_for_scope(cx: &mut Context, scope: vcs::ChangedFileScope) {
    pub struct FileChangeData {
        scope: vcs::ChangedFileScope,
        workspace_root: std::path::PathBuf,
        display_root: Option<std::path::PathBuf>,
        icons: bool,
        style_untracked: Style,
        style_modified: Style,
        style_conflict: Style,
        style_deleted: Style,
        style_renamed: Style,
    }

    fn change_cell(change: Option<&FileChange>, data: &FileChangeData) -> Cell<'static> {
        let Some(change) = change else {
            return Span::raw("—").into();
        };
        let icons = ICONS.load();
        let (plain, icon, label, style) = match change {
            FileChange::Untracked { .. } => (
                "+ untracked",
                icons.vcs().added(),
                "untracked",
                data.style_untracked,
            ),
            FileChange::Added { .. } => (
                "+ added",
                icons.vcs().added(),
                "added",
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

    fn staged_column<'a>(entry: &'a ChangedFileEntry, data: &FileChangeData) -> Cell<'a> {
        change_cell(entry.status.staged.as_ref(), data)
    }
    fn unstaged_column<'a>(entry: &'a ChangedFileEntry, data: &FileChangeData) -> Cell<'a> {
        change_cell(entry.status.unstaged.as_ref(), data)
    }

    fn path_column<'a>(entry: &'a ChangedFileEntry, _data: &FileChangeData) -> Cell<'a> {
        entry.display_path.as_str().into()
    }

    if !scope.path().exists() {
        cx.editor.set_error(|| "Changed file scope does not exist");
        return;
    }

    let workspace_root = workspace::find_workspace_in(scope.path()).0;
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
        PickerColumn::new("staged", staged_column),
        PickerColumn::new("unstaged", unstaged_column),
        PickerColumn::new("path", path_column),
    ];

    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        2, // path
        [],
        FileChangeData {
            scope: scope.clone(),
            workspace_root: workspace_root.clone(),
            display_root: display_root.clone(),
            icons: cx.editor.config().icons,
            style_untracked: added,
            style_modified: modified,
            style_conflict: conflict,
            style_deleted: deleted,
            style_renamed: renamed,
        },
        |cx, entry: &ChangedFileEntry, action| {
            let path_to_open = entry.status.path();
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
    .with_preview(|_editor, entry| Some((entry.status.path().into(), None)))
    .with_custom_preview(crate::ui::change_review::ChangeReviewPreview::new(
        workspace_root.clone(),
        |entry: &ChangedFileEntry| &entry.status,
    ))
    .with_file_updates(
        cx.editor,
        |data| &data.workspace_root,
        |_, editor, data, cancel| {
            let trust_full = editor
                .workspace_trust
                .query(&data.workspace_root, workspace::trust::TrustQuery::Git)
                .is_trusted();
            refresh_changed_files(
                editor.diff_providers.clone(),
                data.scope.clone(),
                trust_full,
                data.display_root.clone(),
                cancel,
            )
        },
    )
    .with_item_key(|entry| entry.status.path().to_path_buf())
    .refresh_on_content_changes()
    .refresh_on_vcs_changes();
    let injector = picker.injector();

    let trust_full = cx
        .editor
        .workspace_trust
        .query(&workspace_root, workspace::trust::TrustQuery::Git)
        .is_trusted();
    let status = cx.editor.status_sender().clone();
    cx.editor.diff_providers.clone().for_each_file_status(
        scope,
        trust_full,
        move |worktree_root, change| match change {
            Ok(change) => injector
                .push(ChangedFileEntry {
                    display_path: display_path(
                        change.change(),
                        display_root.as_deref().unwrap_or(worktree_root),
                    ),
                    status: change,
                })
                .is_ok(),
            Err(err) => {
                status.report_blocking(err);
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
        let mode = editor.mode();
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
            if mode == Mode::Select {
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

    pub(in crate::commands) fn blame(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event == PromptEvent::Validate {
            view::features::blame::request_details(cx.editor);
        }
        Ok(())
    }

    pub(in crate::commands) fn show_change(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event == PromptEvent::Validate {
            super::request_change(cx.editor, cx.jobs);
        }
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn restore_change(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let editor = &mut cx.editor;
        let scrolloff = editor.config().scroll.padding;

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
        view::editing::apply(doc, &transaction, view.id);
        view::editing::commit(doc, view);
        view.ensure_cursor_in_view(doc, scrolloff);
        cx.editor.set_status(format!(
            "Restored {changes} change{}",
            if changes == 1 { "" } else { "s" }
        ));
        Ok(())
    }
}

#[cfg(all(test, feature = "git"))]
mod file_refresh_tests {
    use super::*;

    #[tokio::test]
    async fn refreshed_status_follows_worktree_and_index_changes() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let git = |args: &[&str]| -> anyhow::Result<()> {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env_remove("GIT_DIR")
                .output()?;
            anyhow::ensure!(
                result.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            Ok(())
        };
        git(&["init", "-q"])?;
        let path = dir.path().join("new.txt");
        std::fs::write(&path, "new\n")?;
        let scope = vcs::ChangedFileScope::Directory(dir.path().to_owned());
        let mut task = event::TaskController::new();
        let entries = refresh_changed_files(
            vcs::DiffProviderRegistry::default(),
            scope.clone(),
            false,
            None,
            task.restart(),
        )
        .await?;
        assert_eq!(entries.len(), 1);
        assert!(
            matches!(entries[0].status.change(), FileChange::Untracked { path: found } if found == &path)
        );
        git(&["add", "new.txt"])?;
        let entries = refresh_changed_files(
            vcs::DiffProviderRegistry::default(),
            scope.clone(),
            false,
            None,
            task.restart(),
        )
        .await?;
        assert_eq!(entries.len(), 1);
        assert!(
            matches!(entries[0].status.change(), FileChange::Added { path: found } if found == &path)
        );
        git(&["rm", "--cached", "new.txt"])?;
        std::fs::remove_file(&path)?;
        assert!(refresh_changed_files(
            vcs::DiffProviderRegistry::default(),
            scope,
            false,
            None,
            task.restart()
        )
        .await?
        .is_empty());
        Ok(())
    }
}
