//! Cursor movement, selection extension, and viewport positioning commands.

use view::editing::movement::{self as shared, LineBoundary, LineMotion, ScrollCursor, TextMotion};

use editor_core::movement::{self, Direction, Movement};
use ui_core::keyboard::KeyCode;
use view::{align_view, document::Mode, editor::Motion, Align, Editor};

use super::context::Context;

pub(super) fn move_impl(cx: &mut Context, lines: LineMotion, dir: Direction, behaviour: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    view::editing::movement::move_lines(doc, view, lines, dir, count, behaviour);
}

pub(super) fn text_motion(cx: &mut Context, motion: TextMotion, behaviour: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    shared::move_text(doc, view.id, motion, count, behaviour);
}

pub(super) fn line_boundary(cx: &mut Context, boundary: LineBoundary, extend: bool) {
    let behaviour = if extend || cx.editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };
    text_motion(cx, TextMotion::LineBoundary(boundary), behaviour);
}

pub(super) fn goto_window(cx: &mut Context, align: Align) {
    let count = cx.count();
    let padding = cx.editor.config().scroll.padding;
    let behaviour = if cx.editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };
    let (view, doc) = current!(cx.editor);
    view::editing::movement::goto_window(doc, view, align, count, behaviour, padding);
}

pub(super) fn goto_para_impl(cx: &mut Context, direction: Direction) {
    let count = cx.count();
    cx.editor.apply_motion(move |editor: &mut Editor| {
        let behaviour = if editor.mode() == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        };
        let (view, doc) = current!(editor);
        shared::move_text(
            doc,
            view.id,
            TextMotion::Paragraph(direction),
            count,
            behaviour,
        );
    });
}

pub(super) fn find_char(cx: &mut Context, direction: Direction, inclusive: bool, extend: bool) {
    // Capture the count before the next key resets it.
    let count = cx.count();
    cx.on_next_key(move |cx, event| {
        let ch = match event.code {
            KeyCode::Enter => None,
            KeyCode::Tab => Some('\t'),
            KeyCode::Char(ch) => Some(ch),
            _ => return,
        };
        let behaviour = if extend {
            Movement::Extend
        } else {
            Movement::Move
        };
        let motion: Motion = Box::new(move |editor: &mut Editor| {
            let (view, doc) = current!(editor);
            let text = doc.text().slice(..);
            let selection = doc.selection(view.id).clone().transform(|range| match ch {
                Some(ch) => {
                    movement::find_char(text, range, ch, count, direction, inclusive, behaviour)
                }
                None => {
                    movement::find_line_ending(text, range, count, direction, inclusive, behaviour)
                }
            });
            doc.set_selection(view.id, selection);
        });
        cx.editor.apply_motion(motion);
    })
}

pub(super) fn repeat_last_motion(cx: &mut Context) {
    cx.editor.repeat_last_motion(cx.count())
}

pub fn scroll(cx: &mut Context, offset: usize, direction: Direction, sync_cursor: bool) {
    let behaviour = if cx.editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };
    let cursor = if sync_cursor {
        ScrollCursor::Follow
    } else {
        ScrollCursor::KeepVisible
    };
    cx.editor
        .scroll_view(cx.editor.tree.focus(), offset, direction, cursor, behaviour);
}

pub(super) fn page_up(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc);
    scroll(cx, offset, Direction::Backward, false);
}

pub(super) fn page_down(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc);
    scroll(cx, offset, Direction::Forward, false);
}

pub(super) fn half_page_up(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc) / 2;
    scroll(cx, offset, Direction::Backward, false);
}

pub(super) fn half_page_down(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc) / 2;
    scroll(cx, offset, Direction::Forward, false);
}

pub(super) fn page_cursor_up(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc);
    scroll(cx, offset, Direction::Backward, true);
}

pub(super) fn page_cursor_down(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc);
    scroll(cx, offset, Direction::Forward, true);
}

pub(super) fn page_cursor_half_up(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc) / 2;
    scroll(cx, offset, Direction::Backward, true);
}

pub(super) fn page_cursor_half_down(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let offset = view.inner_height(doc) / 2;
    scroll(cx, offset, Direction::Forward, true);
}

pub(super) fn align_view_top(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    align_view(doc, view, Align::Top);
}

pub(super) fn align_view_center(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    align_view(doc, view, Align::Center);
}

pub(super) fn align_view_bottom(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    align_view(doc, view, Align::Bottom);
}

pub(super) fn align_view_middle(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    view::editing::movement::align_middle(doc, view);
}

pub(super) fn scroll_up(cx: &mut Context) {
    scroll(cx, cx.count(), Direction::Backward, false);
}

pub(super) fn scroll_down(cx: &mut Context) {
    scroll(cx, cx.count(), Direction::Forward, false);
}
