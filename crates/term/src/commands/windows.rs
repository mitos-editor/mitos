//! Window splits, focus, arrangement, and closing.

use super::buffers::typed::buffers_remaining_impl;
use crate::commands::context::Context;
use view::{editor::Action, tree, Editor};

//

pub(super) fn rotate_view(cx: &mut Context) {
    cx.editor.focus_next()
}

pub(super) fn rotate_view_reverse(cx: &mut Context) {
    cx.editor.focus_prev()
}

pub(super) fn jump_view_right(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Right)
}

pub(super) fn jump_view_left(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Left)
}

pub(super) fn jump_view_up(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Up)
}

pub(super) fn jump_view_down(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Down)
}

pub(super) fn swap_view_right(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Right)
}

pub(super) fn swap_view_left(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Left)
}

pub(super) fn swap_view_up(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Up)
}

pub(super) fn swap_view_down(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Down)
}

pub(super) fn transpose_view(cx: &mut Context) {
    cx.editor.transpose_view()
}

/// Open a new split in the given direction specified by the action.
///
/// Maintain the current view (both the cursor's position and view in document).
fn split(editor: &mut Editor, action: Action) {
    let (view, doc) = current!(editor);
    let id = doc.id();
    let selection = doc.selection(view.id).clone();
    let offset = doc.view_offset(view.id);

    editor.switch(id, action);

    // match the selection in the previous view
    let (view, doc) = current!(editor);
    doc.set_selection(view.id, selection);
    // match the view scroll offset (switch doesn't handle this fully
    // since the selection is only matched after the split)
    doc.set_view_offset(view.id, offset);
}

pub(super) fn hsplit(cx: &mut Context) {
    split(cx.editor, Action::HorizontalSplit);
}

pub(super) fn hsplit_new(cx: &mut Context) {
    cx.editor.new_file(Action::HorizontalSplit);
}

pub(super) fn vsplit(cx: &mut Context) {
    split(cx.editor, Action::VerticalSplit);
}

pub(super) fn vsplit_new(cx: &mut Context) {
    cx.editor.new_file(Action::VerticalSplit);
}

pub(super) fn wclose(cx: &mut Context) {
    if cx.editor.tree.views().count() == 1
        && let Err(err) = buffers_remaining_impl(cx.editor)
    {
        cx.editor.set_error(|| err.to_string());
        return;
    }
    let view_id = view!(cx.editor).id;
    // close current split
    cx.editor.close(view_id);
}

pub(super) fn wonly(cx: &mut Context) {
    let views = cx
        .editor
        .tree
        .views()
        .map(|(v, focus)| (v.id, focus))
        .collect::<Vec<_>>();
    for (view_id, focus) in views {
        if !focus {
            cx.editor.close(view_id);
        }
    }
}

pub(super) mod typed {
    //! Typable windows commands.

    use crate::{
        commands::{files::typed::open_impl, windows::split},
        compositor,
        ui::PromptEvent,
    };
    use ::command_line::Args;
    use view::editor::Action;

    #[cold]
    pub(in crate::commands) fn vsplit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        if args.is_empty() {
            split(cx.editor, Action::VerticalSplit);
        } else {
            open_impl(cx, args, Action::VerticalSplit)?;
        }

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn hsplit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        if args.is_empty() {
            split(cx.editor, Action::HorizontalSplit);
        } else {
            open_impl(cx, args, Action::HorizontalSplit)?;
        }

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn vsplit_new(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        cx.editor.new_file(Action::VerticalSplit);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn hsplit_new(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        cx.editor.new_file(Action::HorizontalSplit);

        Ok(())
    }
}
