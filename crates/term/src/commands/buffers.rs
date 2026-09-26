//! Buffer lifecycle and buffer picker commands.

use crate::{
    commands::{context::Context, picker::PathStyleConfig},
    ui::{overlay::overlaid, Picker, PickerColumn},
};
use std::{borrow::Cow, path::Path};
use view::{Document, DocumentId};

pub(super) fn buffer_picker(cx: &mut Context) {
    let current = view!(cx.editor).doc;

    struct BufferMeta<'a> {
        id: DocumentId,
        path: Option<Cow<'a, Path>>,
        is_modified: bool,
        is_current: bool,
        focused_at: std::time::Instant,
    }

    let new_meta = |doc: &Document| BufferMeta {
        id: doc.id(),
        path: doc
            .path()
            .map(ToOwned::to_owned)
            .map(stdx::path::get_relative_path),
        is_modified: doc.is_modified(),
        is_current: doc.id() == current,
        focused_at: doc.focused_at,
    };

    let mut items = cx
        .editor
        .documents
        .values()
        .map(new_meta)
        .collect::<Vec<BufferMeta>>();

    // mru
    items.sort_unstable_by_key(|item| std::cmp::Reverse(item.focused_at));

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
            config.stylize(meta.path.as_deref(), None)
        }),
    ];

    let initial_cursor = if cx
        .editor
        .config()
        .buffer_picker
        .start_position
        .is_previous()
        && !items.is_empty()
    {
        1
    } else {
        0
    };

    let picker = Picker::new(
        columns,
        2,
        items,
        PathStyleConfig::new(cx.editor),
        |cx, meta, action| {
            cx.editor.switch(meta.id, action);
        },
    )
    .with_initial_cursor(initial_cursor)
    .with_preview(|editor, meta| {
        let doc = &editor.documents.get(&meta.id)?;
        let lines = doc.selections().values().next().map(|selection| {
            let cursor_line = selection.primary().cursor_line(doc.text().slice(..));
            (cursor_line, cursor_line)
        });
        Some((meta.id.into(), lines))
    });
    cx.push_layer(Box::new(overlaid(picker)));
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
        cx.block_try_flush_writes()?;

        let (modified_ids, modified_names): (Vec<_>, Vec<_>) = doc_ids
            .iter()
            .filter_map(|&doc_id| match cx.editor.close_document(doc_id, force) {
                Err(CloseError::BufferModified(name)) => Some((doc_id, name)),
                _ => None,
            })
            .unzip();

        if let Some(first) = modified_ids.first() {
            let current = doc!(cx.editor);
            // If the current document is unmodified, and there are modified
            // documents, switch focus to the first modified doc.
            if !modified_ids.contains(&current.id()) {
                cx.editor.switch(*first, Action::Replace);
            }
            bail!(
                "{} unsaved buffer{} remaining: {:?}",
                modified_names.len(),
                if modified_names.len() == 1 { "" } else { "s" },
                modified_names,
            );
        }

        Ok(())
    }

    pub(in crate::commands) fn buffer_gather_paths_impl(
        editor: &mut Editor,
        args: Args,
    ) -> Vec<DocumentId> {
        // No arguments implies current document
        if args.is_empty() {
            let doc_id = view!(editor).doc;
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
                .map(|view| &view.0.doc)
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
