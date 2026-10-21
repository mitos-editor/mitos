//! Cursor movement, selection extension, and viewport positioning commands.

use view::editing::movement::{
    self as shared, LineBoundary, LineMotion, ScrollCursor, TextMotion, WordMotionTarget,
};

use editor_core::movement::{self, Direction, Movement};
use ui_core::keyboard::KeyCode;
use view::{align_view, document::Mode, editor::Motion, Align, Editor};

use super::context::Context;

fn move_impl(cx: &mut Context, lines: LineMotion, dir: Direction, behaviour: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    view::editing::movement::move_lines(doc, view, lines, dir, count, behaviour);
}

fn move_char_impl(cx: &mut Context, dir: Direction, behaviour: Movement) {
    text_motion(cx, TextMotion::Character(dir), behaviour);
}

fn text_motion(cx: &mut Context, motion: TextMotion, behaviour: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    shared::move_text(doc, view.id, motion, count, behaviour);
}

pub(super) fn move_char_left(cx: &mut Context) {
    move_char_impl(cx, Direction::Backward, Movement::Move)
}

pub(super) fn move_char_right(cx: &mut Context) {
    move_char_impl(cx, Direction::Forward, Movement::Move)
}

pub(super) fn move_line_up(cx: &mut Context) {
    move_impl(
        cx,
        LineMotion::Document,
        Direction::Backward,
        Movement::Move,
    )
}

pub(super) fn move_line_down(cx: &mut Context) {
    move_impl(cx, LineMotion::Document, Direction::Forward, Movement::Move)
}

pub(super) fn move_visual_line_up(cx: &mut Context) {
    move_impl(cx, LineMotion::Visual, Direction::Backward, Movement::Move)
}

pub(super) fn move_visual_line_down(cx: &mut Context) {
    move_impl(cx, LineMotion::Visual, Direction::Forward, Movement::Move)
}

pub(super) fn extend_char_left(cx: &mut Context) {
    move_char_impl(cx, Direction::Backward, Movement::Extend)
}

pub(super) fn extend_char_right(cx: &mut Context) {
    move_char_impl(cx, Direction::Forward, Movement::Extend)
}

pub(super) fn extend_line_up(cx: &mut Context) {
    move_impl(
        cx,
        LineMotion::Document,
        Direction::Backward,
        Movement::Extend,
    )
}

pub(super) fn extend_line_down(cx: &mut Context) {
    move_impl(
        cx,
        LineMotion::Document,
        Direction::Forward,
        Movement::Extend,
    )
}

pub(super) fn extend_visual_line_up(cx: &mut Context) {
    move_impl(
        cx,
        LineMotion::Visual,
        Direction::Backward,
        Movement::Extend,
    )
}

pub(super) fn extend_visual_line_down(cx: &mut Context) {
    move_impl(cx, LineMotion::Visual, Direction::Forward, Movement::Extend)
}

fn line_boundary(cx: &mut Context, boundary: LineBoundary, extend: bool) {
    let behaviour = if extend || cx.editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };
    text_motion(cx, TextMotion::LineBoundary(boundary), behaviour);
}

pub(super) fn goto_line_end(cx: &mut Context) {
    line_boundary(cx, LineBoundary::End, false);
}
pub(super) fn extend_to_line_end(cx: &mut Context) {
    line_boundary(cx, LineBoundary::End, true);
}

pub(super) fn goto_line_end_newline(cx: &mut Context) {
    line_boundary(cx, LineBoundary::Ending, false);
}
pub(super) fn extend_to_line_end_newline(cx: &mut Context) {
    line_boundary(cx, LineBoundary::Ending, true);
}

pub(super) fn goto_line_start(cx: &mut Context) {
    line_boundary(cx, LineBoundary::Start, false);
}
pub(super) fn extend_to_line_start(cx: &mut Context) {
    line_boundary(cx, LineBoundary::Start, true);
}

pub(super) fn goto_first_nonwhitespace(cx: &mut Context) {
    line_boundary(cx, LineBoundary::FirstNonWhitespace, false);
}
pub(super) fn extend_to_first_nonwhitespace(cx: &mut Context) {
    line_boundary(cx, LineBoundary::FirstNonWhitespace, true);
}

fn goto_window(cx: &mut Context, align: Align) {
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

pub(super) fn goto_window_top(cx: &mut Context) {
    goto_window(cx, Align::Top)
}

pub(super) fn goto_window_center(cx: &mut Context) {
    goto_window(cx, Align::Center)
}

pub(super) fn goto_window_bottom(cx: &mut Context) {
    goto_window(cx, Align::Bottom)
}

fn move_word_impl(cx: &mut Context, motion: WordMotionTarget) {
    text_motion(cx, TextMotion::Word(motion), Movement::Move);
}

pub(super) fn move_next_word_start(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::NextWordStart)
}

pub(super) fn move_prev_word_start(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::PrevWordStart)
}

pub(super) fn move_prev_word_end(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::PrevWordEnd)
}

pub(super) fn move_next_word_end(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::NextWordEnd)
}

pub(super) fn move_next_long_word_start(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::NextLongWordStart)
}

pub(super) fn move_prev_long_word_start(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::PrevLongWordStart)
}

pub(super) fn move_prev_long_word_end(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::PrevLongWordEnd)
}

pub(super) fn move_next_long_word_end(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::NextLongWordEnd)
}

pub(super) fn move_next_sub_word_start(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::NextSubWordStart)
}

pub(super) fn move_prev_sub_word_start(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::PrevSubWordStart)
}

pub(super) fn move_prev_sub_word_end(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::PrevSubWordEnd)
}

pub(super) fn move_next_sub_word_end(cx: &mut Context) {
    move_word_impl(cx, WordMotionTarget::NextSubWordEnd)
}

fn goto_para_impl(cx: &mut Context, direction: Direction) {
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

pub(super) fn goto_prev_paragraph(cx: &mut Context) {
    goto_para_impl(cx, Direction::Backward)
}

pub(super) fn goto_next_paragraph(cx: &mut Context) {
    goto_para_impl(cx, Direction::Forward)
}

fn extend_word_impl(cx: &mut Context, motion: WordMotionTarget) {
    text_motion(cx, TextMotion::Word(motion), Movement::Extend);
}

pub(super) fn extend_next_word_start(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::NextWordStart)
}

pub(super) fn extend_prev_word_start(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::PrevWordStart)
}

pub(super) fn extend_next_word_end(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::NextWordEnd)
}

pub(super) fn extend_prev_word_end(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::PrevWordEnd)
}

pub(super) fn extend_next_long_word_start(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::NextLongWordStart)
}

pub(super) fn extend_prev_long_word_start(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::PrevLongWordStart)
}

pub(super) fn extend_prev_long_word_end(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::PrevLongWordEnd)
}

pub(super) fn extend_next_long_word_end(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::NextLongWordEnd)
}

pub(super) fn extend_next_sub_word_start(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::NextSubWordStart)
}

pub(super) fn extend_prev_sub_word_start(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::PrevSubWordStart)
}

pub(super) fn extend_prev_sub_word_end(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::PrevSubWordEnd)
}

pub(super) fn extend_next_sub_word_end(cx: &mut Context) {
    extend_word_impl(cx, WordMotionTarget::NextSubWordEnd)
}

fn find_char(cx: &mut Context, direction: Direction, inclusive: bool, extend: bool) {
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

pub(super) fn find_till_char(cx: &mut Context) {
    find_char(cx, Direction::Forward, false, false);
}

pub(super) fn find_next_char(cx: &mut Context) {
    find_char(cx, Direction::Forward, true, false)
}

pub(super) fn extend_till_char(cx: &mut Context) {
    find_char(cx, Direction::Forward, false, true)
}

pub(super) fn extend_next_char(cx: &mut Context) {
    find_char(cx, Direction::Forward, true, true)
}

pub(super) fn till_prev_char(cx: &mut Context) {
    find_char(cx, Direction::Backward, false, false)
}

pub(super) fn find_prev_char(cx: &mut Context) {
    find_char(cx, Direction::Backward, true, false)
}

pub(super) fn extend_till_prev_char(cx: &mut Context) {
    find_char(cx, Direction::Backward, false, true)
}

pub(super) fn extend_prev_char(cx: &mut Context) {
    find_char(cx, Direction::Backward, true, true)
}

pub(super) fn repeat_last_motion(cx: &mut Context) {
    cx.editor.repeat_last_motion(cx.count())
}

pub fn scroll(cx: &mut Context, offset: usize, direction: Direction, sync_cursor: bool) {
    let padding = cx.editor.config().scroll.padding;
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
    let (view, doc) = current!(cx.editor);
    view::editing::movement::scroll(doc, view, offset, direction, cursor, behaviour, padding);
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
