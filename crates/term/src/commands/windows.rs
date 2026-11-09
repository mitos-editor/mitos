//! Window splits, focus, arrangement, and closing.

use super::buffers::typed::buffers_remaining_impl;
use crate::commands::context::Context;
use view::{editor::Action, tree, Editor};

//

pub(super) fn rotate_view(cx: &mut Context) {
    rotate(cx, false);
}

pub(super) fn rotate_view_reverse(cx: &mut Context) {
    rotate(cx, true);
}

fn rotate(cx: &mut Context, reverse: bool) {
    let boundary = if reverse {
        cx.editor.tree.views().next().map(|(view, _)| view.id)
    } else {
        cx.editor.tree.views().last().map(|(view, _)| view.id)
    };
    if cx.interaction.sidebar_visible && boundary == Some(cx.editor.tree.focus()) {
        cx.callback.push(Box::new(|compositor, cx| {
            if let Some(view) = compositor.find::<crate::ui::EditorView>() {
                view.focus_visible_sidebar(cx.editor);
            }
        }));
    } else if reverse {
        cx.editor.focus_prev();
    } else {
        cx.editor.focus_next();
    }
}

pub(super) fn jump_view_right(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Right)
}

pub(super) fn jump_view_left(cx: &mut Context) {
    let previous = cx.editor.tree.focus();
    cx.editor.focus_direction(tree::Direction::Left);
    if cx.editor.tree.focus() == previous {
        cx.callback.push(Box::new(|compositor, cx| {
            if let Some(view) = compositor.find::<crate::ui::EditorView>() {
                view.focus_visible_sidebar(cx.editor);
            }
        }));
    }
}

pub(super) fn toggle_sidebar(cx: &mut Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        if let Some(view) = compositor.find::<crate::ui::EditorView>() {
            view.toggle_sidebar(cx.editor);
        }
    }));
}

pub(super) fn focus_sidebar(cx: &mut Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        if let Some(view) = compositor.find::<crate::ui::EditorView>() {
            view.focus_sidebar(cx.editor);
        }
    }));
}

pub(super) fn switch_sidebar_focus(cx: &mut Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        if let Some(view) = compositor.find::<crate::ui::EditorView>() {
            view.switch_sidebar_focus(cx.editor);
        }
    }));
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

fn resize_view(cx: &mut Context, dimension: tree::Dimension, sign: isize) {
    let cells = isize::try_from(cx.count()).unwrap_or(isize::MAX);
    cx.editor.resize_view(dimension, sign.saturating_mul(cells));
}

pub(super) fn grow_view_width(cx: &mut Context) {
    resize_view(cx, tree::Dimension::Width, 1)
}

pub(super) fn shrink_view_width(cx: &mut Context) {
    resize_view(cx, tree::Dimension::Width, -1)
}

pub(super) fn grow_view_height(cx: &mut Context) {
    resize_view(cx, tree::Dimension::Height, 1)
}

pub(super) fn shrink_view_height(cx: &mut Context) {
    resize_view(cx, tree::Dimension::Height, -1)
}

pub(super) fn equalize_views(cx: &mut Context) {
    cx.editor.equalize_views()
}

pub(super) fn resize_mode(cx: &mut Context) {
    let count = cx.count;
    cx.callback.push(Box::new(move |compositor, cx| {
        if let Some(view) = compositor.find::<crate::ui::EditorView>() {
            view.start_resize(cx.editor, count);
        }
    }));
}

fn resize_view_direction(cx: &mut Context, direction: tree::Direction) {
    let default = match direction {
        tree::Direction::Left | tree::Direction::Right => 5,
        tree::Direction::Up | tree::Direction::Down => 2,
    };
    let cells = cx.count.map_or(default, |count| count.get());
    cx.callback.push(Box::new(move |compositor, cx| {
        if let Some(view) = compositor.find::<crate::ui::EditorView>() {
            view.resize_direction(cx.editor, direction, cells);
        }
    }));
}

pub(super) fn resize_view_left(cx: &mut Context) {
    resize_view_direction(cx, tree::Direction::Left)
}

pub(super) fn resize_view_right(cx: &mut Context) {
    resize_view_direction(cx, tree::Direction::Right)
}

pub(super) fn resize_view_up(cx: &mut Context) {
    resize_view_direction(cx, tree::Direction::Up)
}

pub(super) fn resize_view_down(cx: &mut Context) {
    resize_view_direction(cx, tree::Direction::Down)
}

/// Open a new split in the given direction specified by the action.
///
/// Maintain the current view (both the cursor's position and view in document).
fn split(editor: &mut Editor, action: Action) {
    let id = current_ref!(editor).1.id();
    editor.switch(id, action);
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
    };
    use ::command_line::Args;
    use view::editor::Action;

    #[cold]
    pub(in crate::commands) fn vsplit(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
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
    ) -> anyhow::Result<()> {
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
    ) -> anyhow::Result<()> {
        cx.editor.new_file(Action::VerticalSplit);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn hsplit_new(
        cx: &mut compositor::Context,
        _args: Args,
    ) -> anyhow::Result<()> {
        cx.editor.new_file(Action::HorizontalSplit);

        Ok(())
    }
}
