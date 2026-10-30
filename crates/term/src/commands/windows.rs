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
    let sizes = cx.editor.tree.split_sizes();
    resize_mode_next_key(cx, sizes, cx.count);
}

fn resize_mode_next_key(
    cx: &mut Context,
    sizes: tree::SplitSizes,
    count: Option<std::num::NonZeroUsize>,
) {
    use crate::key;
    use std::num::NonZeroUsize;
    use view::info::Info;

    cx.editor.autoinfo = Some(Info::new(
        "Resize split",
        &[
            ("h, left", "Resize split left"),
            ("l, right", "Resize split right"),
            ("j, down", "Resize split down"),
            ("k, up", "Resize split up"),
            ("=", "Make all splits the same size"),
            ("ret", "Confirm resizing"),
            ("esc", "Cancel resizing"),
        ],
    ));
    cx.on_next_key(move |cx, event| {
        match event {
            // Mouse input and paste dismiss next-key callbacks with a null key.
            key!(Enter) | key!(Esc) | key!(Null) => {
                if event != key!(Enter) {
                    cx.editor.restore_split_sizes(sizes);
                }
                cx.editor.autoinfo = None;
                cx.editor.count = None;
                return;
            }
            key!(digit @ '0'..='9') if count.is_some() || digit != '0' => {
                let next =
                    count.map_or(0, NonZeroUsize::get) * 10 + digit.to_digit(10).unwrap() as usize;
                let count = if next <= 100_000_000 {
                    NonZeroUsize::new(next)
                } else {
                    count
                };
                cx.editor.count = count;
                resize_mode_next_key(cx, sizes, count);
                return;
            }
            _ => {}
        }
        cx.count = count;
        match event {
            key!('h') | key!(Left) => resize_view_left(cx),
            key!('l') | key!(Right) => resize_view_right(cx),
            key!('j') | key!(Down) => resize_view_down(cx),
            key!('k') | key!(Up) => resize_view_up(cx),
            key!('=') => equalize_views(cx),
            _ => {}
        }
        cx.editor.count = None;
        resize_mode_next_key(cx, sizes, None);
    });
}

fn resize_view_direction(cx: &mut Context, direction: tree::Direction) {
    let default = match direction {
        tree::Direction::Left | tree::Direction::Right => 5,
        tree::Direction::Up | tree::Direction::Down => 2,
    };
    let cells = cx.count.map_or(default, |count| count.get());
    cx.editor.resize_view_direction(direction, cells);
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
