//! Buffer lifecycle and buffer picker commands.

use crate::{
    commands::{context::Context, picker::PathStyleConfig},
    keymap::component::PickerItemAction,
    ui::{overlay::overlaid, picker::PickerCallbackResult, Picker, PickerColumn},
};
use std::{borrow::Cow, path::Path};
use view::document::DocumentKind;
use view::{DocumentId, Editor};

#[derive(PartialEq)]
struct BufferMeta {
    id: DocumentId,
    name: String,
    kind: DocumentKind,
    path: Option<Cow<'static, Path>>,
    is_modified: bool,
    is_current: bool,
    focused_at: std::time::Instant,
}

fn buffer_items(editor: &Editor) -> Vec<BufferMeta> {
    let current = view!(editor).document_id();
    let mut items: Vec<_> = editor
        .documents()
        .map(|doc| BufferMeta {
            id: doc.id(),
            name: doc.display_name().into_owned(),
            kind: doc.display_metadata().kind,
            path: doc
                .display_metadata()
                .source_path
                .map(|path| std::path::PathBuf::from(doc.display_metadata().format_path(path)))
                .map(stdx::path::get_relative_path),
            is_modified: doc.is_modified(),
            is_current: doc.id() == current,
            focused_at: doc.focused_at,
        })
        .collect();
    // MRU order is stable when an inactive buffer is closed.
    items.sort_unstable_by_key(|item| std::cmp::Reverse(item.focused_at));
    items
}

fn close_selected_buffer(
    cx: &mut crate::compositor::Context,
    meta: Option<&BufferMeta>,
    action: PickerItemAction,
) -> PickerCallbackResult<BufferMeta, PathStyleConfig> {
    match (action, meta) {
        (PickerItemAction::CloseBuffer, Some(meta)) => {
            let doc_id = meta.id;
            if let Err(error) =
                cx.after_writes(move |editor| match editor.close_document(doc_id, false) {
                    Ok(()) | Err(view::editor::CloseError::DoesNotExist) => Ok(()),
                    Err(view::editor::CloseError::BufferModified(name)) => {
                        anyhow::bail!("Buffer '{name}' has unsaved changes")
                    }
                    Err(view::editor::CloseError::SaveError(error)) => Err(error),
                })
            {
                cx.editor.set_error(|| error.to_string());
                return PickerCallbackResult::KeepOpen;
            }
            PickerCallbackResult::Refresh {
                options: buffer_items(cx.editor),
                editor_data: PathStyleConfig::new(cx.editor),
            }
        }
        _ => PickerCallbackResult::KeepOpen,
    }
}

pub(super) fn buffer_picker(cx: &mut Context) {
    cx.push_layer(Box::new(overlaid(buffer_picker_for_editor(cx.editor))));
}

fn buffer_picker_for_editor(editor: &Editor) -> Picker<BufferMeta, PathStyleConfig> {
    let items = buffer_items(editor);

    let columns = [
        PickerColumn::new("id", |meta: &BufferMeta, _| meta.id.to_string().into()),
        PickerColumn::new("flags", |meta: &BufferMeta, _| {
            let mut flags = String::new();
            if meta.is_modified {
                flags.push('+');
            }
            if meta.is_current {
                flags.push('*');
            }
            flags.into()
        }),
        PickerColumn::new("path", |meta: &BufferMeta, config: &PathStyleConfig| {
            if meta.path.is_none() {
                meta.name.as_str().into()
            } else {
                let mut cell = config.stylize(meta.path.as_deref(), None);
                let suffix = meta.kind.suffix();
                if !suffix.is_empty() {
                    cell.content.lines[0]
                        .spans
                        .push(tui::text::Span::raw(suffix.into_owned()));
                }
                cell
            }
        })
        .with_prefix(|meta, config| config.icon(meta.path.as_deref())),
    ];

    let initial_cursor =
        if editor.config().buffer_picker.start_position.is_previous() && !items.is_empty() {
            1
        } else {
            0
        };

    Picker::new(
        editor.redraw().clone(),
        columns,
        2,
        items,
        PathStyleConfig::new(editor),
        |cx, meta, action| {
            cx.editor.switch(meta.id, action);
        },
    )
    .with_title("Buffers")
    .with_item_key(|meta| meta.id)
    .with_editor_updates(|cx| Some(buffer_items(cx.editor)))
    .with_item_actions(|maps| &maps.buffer_picker, close_selected_buffer)
    .with_initial_cursor(initial_cursor)
    .with_preview(|editor, meta| {
        let doc = &editor.documents.get(&meta.id)?;
        let lines = doc.selections().next().map(|selection| {
            let cursor_line = selection.primary().cursor_line(doc.text().slice(..));
            (cursor_line, cursor_line)
        });
        Some((meta.id.into(), lines))
    })
}

#[cfg(all(test, feature = "integration"))]
mod tests {
    use super::*;
    use crate::ui::picker::test::{application, items, until};
    use view::editor::Action;

    #[tokio::test]
    async fn buffer_picker_tracks_lifecycle_without_changing_selection() -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("first.txt"), "first")?;
        std::fs::write(dir.path().join("second.txt"), "second")?;
        let first = app
            .editor
            .open(&dir.path().join("first.txt"), Action::Replace)?;
        let mut picker = buffer_picker_for_editor(&app.editor);
        until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
        let selected = picker.selection().unwrap().id;
        let second = app
            .editor
            .open(&dir.path().join("second.txt"), Action::Replace)?;
        let count = app.editor.documents().count();
        until(&mut picker, &mut app, |picker| {
            items(picker).count() == count
        })
        .await?;
        assert_eq!(picker.selection().unwrap().id, selected);
        assert!(app.editor.close_document(second, false).is_ok());
        until(&mut picker, &mut app, |picker| {
            items(picker).all(|item| item.id != second)
        })
        .await?;
        assert_eq!(picker.selection().unwrap().id, selected);
        assert!(app.editor.document(first).is_some());
        Ok(())
    }
}

pub(super) mod typed {
    //! Typable buffers commands.

    use crate::{commands::navigation::goto_buffer, compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::bail;
    use editor_core::movement::Direction;
    use std::{collections::HashSet, path::Path};
    use view::{
        editor::{Action, CloseError},
        DocumentId, Editor,
    };

    pub(in crate::commands) fn buffer_close_by_ids_impl(
        cx: &mut compositor::Context,
        doc_ids: &[DocumentId],
        force: bool,
    ) -> anyhow::Result<()> {
        let doc_ids = doc_ids.to_vec();
        cx.after_writes(move |editor| {
            let (modified_ids, modified_names): (Vec<_>, Vec<_>) = doc_ids
                .iter()
                .filter_map(|&doc_id| match editor.close_document(doc_id, force) {
                    Err(CloseError::BufferModified(name)) => Some((doc_id, name)),
                    _ => None,
                })
                .unzip();

            if let Some(first) = modified_ids.first() {
                let current = doc!(editor);
                // If the current document is unmodified, and there are modified
                // documents, switch focus to the first modified doc.
                if !modified_ids.contains(&current.id()) {
                    editor.switch(*first, Action::Replace);
                }
                bail!(
                    "{} unsaved buffer{} remaining: {:?}",
                    modified_names.len(),
                    if modified_names.len() == 1 { "" } else { "s" },
                    modified_names,
                );
            }

            Ok(())
        })
    }

    pub(in crate::commands) fn buffer_gather_paths_impl(
        editor: &mut Editor,
        args: Args,
    ) -> Vec<DocumentId> {
        // No arguments implies current document
        if args.is_empty() {
            let doc_id = view!(editor).document_id();
            return vec![doc_id];
        }

        let mut nonexistent_buffers = vec![];
        let mut document_ids = vec![];
        for arg in args {
            let doc_id = editor.documents().find_map(|doc| {
                let arg_path = Some(Path::new(arg.as_ref()));
                if doc.path() == arg_path
                    || doc.relative_path() == arg_path
                    || doc.display_path() == arg_path
                {
                    Some(doc.id())
                } else {
                    None
                }
            });

            match doc_id {
                Some(doc_id) => document_ids.push(doc_id),
                None => nonexistent_buffers.push(format!("'{}'", arg)),
            }
        }

        if !nonexistent_buffers.is_empty() {
            editor.set_error(|| {
                format!(
                    "cannot close non-existent buffers: {}",
                    nonexistent_buffers.join(", ")
                )
            });
        }

        document_ids
    }

    #[cold]
    pub(in crate::commands) fn buffer_close(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let document_ids = buffer_gather_paths_impl(cx.editor, args);
        buffer_close_by_ids_impl(cx, &document_ids, false)
    }

    #[cold]
    pub(in crate::commands) fn force_buffer_close(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let document_ids = buffer_gather_paths_impl(cx.editor, args);
        buffer_close_by_ids_impl(cx, &document_ids, true)
    }

    fn buffer_gather_others_impl(editor: &mut Editor, skip_visible: bool) -> Vec<DocumentId> {
        if skip_visible {
            let visible_document_ids = editor
                .tree
                .views()
                .map(|(view, _)| view.document_id())
                .collect::<HashSet<_>>();
            editor
                .documents()
                .map(|doc| doc.id())
                .filter(|doc_id| !visible_document_ids.contains(doc_id))
                .collect()
        } else {
            let current_document = &doc!(editor).id();
            editor
                .documents()
                .map(|doc| doc.id())
                .filter(|doc_id| doc_id != current_document)
                .collect()
        }
    }

    #[cold]
    pub(in crate::commands) fn buffer_close_others(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let document_ids = buffer_gather_others_impl(cx.editor, args.has_flag("skip-visible"));
        buffer_close_by_ids_impl(cx, &document_ids, false)
    }

    #[cold]
    pub(in crate::commands) fn force_buffer_close_others(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let document_ids = buffer_gather_others_impl(cx.editor, args.has_flag("skip-visible"));
        buffer_close_by_ids_impl(cx, &document_ids, true)
    }

    fn buffer_gather_all_impl(editor: &mut Editor) -> Vec<DocumentId> {
        editor.documents().map(|doc| doc.id()).collect()
    }

    #[cold]
    pub(in crate::commands) fn buffer_close_all(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let document_ids = buffer_gather_all_impl(cx.editor);
        buffer_close_by_ids_impl(cx, &document_ids, false)
    }

    #[cold]
    pub(in crate::commands) fn force_buffer_close_all(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let document_ids = buffer_gather_all_impl(cx.editor);
        buffer_close_by_ids_impl(cx, &document_ids, true)
    }

    #[cold]
    pub(in crate::commands) fn buffer_next(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        goto_buffer(cx.editor, Direction::Forward, 1);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn buffer_previous(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        goto_buffer(cx.editor, Direction::Backward, 1);
        Ok(())
    }

    /// Results in an error if there are modified buffers remaining and sets editor
    /// error, otherwise returns `Ok(())`. If the current document is unmodified,
    /// and there are modified documents, switches focus to one of them.
    pub(in crate::commands) fn buffers_remaining_impl(editor: &mut Editor) -> anyhow::Result<()> {
        let modified_ids: Vec<_> = editor
            .documents()
            .filter(|doc| doc.is_modified())
            .map(|doc| doc.id())
            .collect();

        if let Some(first) = modified_ids.first() {
            let current = doc!(editor);
            // If the current document is unmodified, and there are modified
            // documents, switch focus to the first modified doc.
            if !modified_ids.contains(&current.id()) {
                editor.switch(*first, Action::Replace);
            }

            let modified_names: Vec<_> = modified_ids
                .iter()
                .map(|doc_id| doc!(editor, doc_id).display_name())
                .collect();

            bail!(
                "{} unsaved buffer{} remaining: {:?}",
                modified_names.len(),
                if modified_names.len() == 1 { "" } else { "s" },
                modified_names,
            );
        }
        Ok(())
    }
}
