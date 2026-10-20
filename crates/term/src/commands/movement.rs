//! Cursor movement, selection extension, and viewport positioning commands.

use view::editing::movement::{LineMotion, ScrollCursor};

use editor_core::{
    graphemes::{self, next_grapheme_boundary},
    line_ending::line_end_char_index,
    movement::{self, move_horizontally, Direction, Movement},
    search, Range, RopeSlice,
};
use stdx::rope::RopeSliceExt;
use ui_core::keyboard::KeyCode;
use view::{align_view, document::Mode, editor::Motion, view::View, Align, Document, Editor};

use super::context::Context;

fn move_impl(cx: &mut Context, lines: LineMotion, dir: Direction, behaviour: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    view::editing::movement::move_lines(doc, view, lines, dir, count, behaviour);
}

fn move_char_impl(cx: &mut Context, dir: Direction, behaviour: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| move_horizontally(text, range, dir, count, behaviour));
    doc.set_selection(view.id, selection);
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

fn goto_line_end_impl(view: &mut View, doc: &mut Document, movement: Movement) {
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let line = range.cursor_line(text);
        let line_start = text.line_to_char(line);

        let pos = graphemes::prev_grapheme_boundary(text, line_end_char_index(&text, line))
            .max(line_start);

        range.put_cursor(text, pos, movement == Movement::Extend)
    });
    doc.set_selection(view.id, selection);
}

pub(super) fn goto_line_end(cx: &mut Context) {
    let mode = cx.editor.mode();
    let (view, doc) = current!(cx.editor);
    goto_line_end_impl(
        view,
        doc,
        if mode == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        },
    )
}

pub(super) fn extend_to_line_end(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    goto_line_end_impl(view, doc, Movement::Extend)
}

fn goto_line_end_newline_impl(view: &mut View, doc: &mut Document, movement: Movement) {
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let line = range.cursor_line(text);
        let pos = line_end_char_index(&text, line);

        range.put_cursor(text, pos, movement == Movement::Extend)
    });
    doc.set_selection(view.id, selection);
}

pub(super) fn goto_line_end_newline(cx: &mut Context) {
    let mode = cx.editor.mode();
    let (view, doc) = current!(cx.editor);
    goto_line_end_newline_impl(
        view,
        doc,
        if mode == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        },
    )
}

pub(super) fn extend_to_line_end_newline(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    goto_line_end_newline_impl(view, doc, Movement::Extend)
}

fn goto_line_start_impl(view: &mut View, doc: &mut Document, movement: Movement) {
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let line = range.cursor_line(text);

        // adjust to start of the line
        let pos = text.line_to_char(line);
        range.put_cursor(text, pos, movement == Movement::Extend)
    });
    doc.set_selection(view.id, selection);
}

pub(super) fn goto_line_start(cx: &mut Context) {
    let mode = cx.editor.mode();
    let (view, doc) = current!(cx.editor);
    goto_line_start_impl(
        view,
        doc,
        if mode == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        },
    )
}

pub(super) fn extend_to_line_start(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    goto_line_start_impl(view, doc, Movement::Extend)
}

pub(super) fn goto_first_nonwhitespace(cx: &mut Context) {
    let mode = cx.editor.mode();
    let (view, doc) = current!(cx.editor);

    goto_first_nonwhitespace_impl(
        view,
        doc,
        if mode == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        },
    )
}

pub(super) fn extend_to_first_nonwhitespace(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    goto_first_nonwhitespace_impl(view, doc, Movement::Extend)
}

fn goto_first_nonwhitespace_impl(view: &mut View, doc: &mut Document, movement: Movement) {
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let line = range.cursor_line(text);

        if let Some(pos) = text.line(line).first_non_whitespace_char() {
            let pos = pos + text.line_to_char(line);
            range.put_cursor(text, pos, movement == Movement::Extend)
        } else {
            range
        }
    });
    doc.set_selection(view.id, selection);
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

fn move_word_impl<F>(cx: &mut Context, move_fn: F)
where
    F: Fn(RopeSlice, Range, usize) -> Range,
{
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| move_fn(text, range, count));
    doc.set_selection(view.id, selection);
}

pub(super) fn move_next_word_start(cx: &mut Context) {
    move_word_impl(cx, movement::move_next_word_start)
}

pub(super) fn move_prev_word_start(cx: &mut Context) {
    move_word_impl(cx, movement::move_prev_word_start)
}

pub(super) fn move_prev_word_end(cx: &mut Context) {
    move_word_impl(cx, movement::move_prev_word_end)
}

pub(super) fn move_next_word_end(cx: &mut Context) {
    move_word_impl(cx, movement::move_next_word_end)
}

pub(super) fn move_next_long_word_start(cx: &mut Context) {
    move_word_impl(cx, movement::move_next_long_word_start)
}

pub(super) fn move_prev_long_word_start(cx: &mut Context) {
    move_word_impl(cx, movement::move_prev_long_word_start)
}

pub(super) fn move_prev_long_word_end(cx: &mut Context) {
    move_word_impl(cx, movement::move_prev_long_word_end)
}

pub(super) fn move_next_long_word_end(cx: &mut Context) {
    move_word_impl(cx, movement::move_next_long_word_end)
}

pub(super) fn move_next_sub_word_start(cx: &mut Context) {
    move_word_impl(cx, movement::move_next_sub_word_start)
}

pub(super) fn move_prev_sub_word_start(cx: &mut Context) {
    move_word_impl(cx, movement::move_prev_sub_word_start)
}

pub(super) fn move_prev_sub_word_end(cx: &mut Context) {
    move_word_impl(cx, movement::move_prev_sub_word_end)
}

pub(super) fn move_next_sub_word_end(cx: &mut Context) {
    move_word_impl(cx, movement::move_next_sub_word_end)
}

fn goto_para_impl<F>(cx: &mut Context, move_fn: F)
where
    F: Fn(RopeSlice, Range, usize, Movement) -> Range + 'static,
{
    let count = cx.count();
    let motion = move |editor: &mut Editor| {
        let mode = editor.mode();
        let (view, doc) = current!(editor);
        let text = doc.text().slice(..);
        let behavior = if mode == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        };

        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| move_fn(text, range, count, behavior));
        doc.set_selection(view.id, selection);
    };
    cx.editor.apply_motion(motion)
}

pub(super) fn goto_prev_paragraph(cx: &mut Context) {
    goto_para_impl(cx, movement::move_prev_paragraph)
}

pub(super) fn goto_next_paragraph(cx: &mut Context) {
    goto_para_impl(cx, movement::move_next_paragraph)
}

fn extend_word_impl<F>(cx: &mut Context, extend_fn: F)
where
    F: Fn(RopeSlice, Range, usize) -> Range,
{
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let word = extend_fn(text, range, count);
        let pos = word.cursor(text);
        range.put_cursor(text, pos, true)
    });
    doc.set_selection(view.id, selection);
}

pub(super) fn extend_next_word_start(cx: &mut Context) {
    extend_word_impl(cx, movement::move_next_word_start)
}

pub(super) fn extend_prev_word_start(cx: &mut Context) {
    extend_word_impl(cx, movement::move_prev_word_start)
}

pub(super) fn extend_next_word_end(cx: &mut Context) {
    extend_word_impl(cx, movement::move_next_word_end)
}

pub(super) fn extend_prev_word_end(cx: &mut Context) {
    extend_word_impl(cx, movement::move_prev_word_end)
}

pub(super) fn extend_next_long_word_start(cx: &mut Context) {
    extend_word_impl(cx, movement::move_next_long_word_start)
}

pub(super) fn extend_prev_long_word_start(cx: &mut Context) {
    extend_word_impl(cx, movement::move_prev_long_word_start)
}

pub(super) fn extend_prev_long_word_end(cx: &mut Context) {
    extend_word_impl(cx, movement::move_prev_long_word_end)
}

pub(super) fn extend_next_long_word_end(cx: &mut Context) {
    extend_word_impl(cx, movement::move_next_long_word_end)
}

pub(super) fn extend_next_sub_word_start(cx: &mut Context) {
    extend_word_impl(cx, movement::move_next_sub_word_start)
}

pub(super) fn extend_prev_sub_word_start(cx: &mut Context) {
    extend_word_impl(cx, movement::move_prev_sub_word_start)
}

pub(super) fn extend_prev_sub_word_end(cx: &mut Context) {
    extend_word_impl(cx, movement::move_prev_sub_word_end)
}

pub(super) fn extend_next_sub_word_end(cx: &mut Context) {
    extend_word_impl(cx, movement::move_next_sub_word_end)
}

/// Separate branch to find_char designed only for `<ret>` char.
//
// This is necessary because the one document can have different line endings inside. And we
// cannot predict what character to find when <ret> is pressed. On the current line it can be `lf`
// but on the next line it can be `crlf`. That's why [`find_char_impl`] cannot be applied here.
fn find_char_line_ending_motion(
    editor: &mut Editor,
    count: usize,
    direction: Direction,
    inclusive: bool,
    extend: bool,
) {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let cursor_anchor = range.cursor(text);
        let cursor_head = next_grapheme_boundary(text, cursor_anchor);
        let cursor_line = range.cursor_line(text);

        let pos = match direction {
            Direction::Forward => {
                let line_end = line_end_char_index(&text, cursor_line);
                let on_edge = if inclusive {
                    line_end == cursor_anchor
                } else {
                    line_end == cursor_head || line_end == cursor_anchor
                };
                let line = cursor_line + count - 1 + on_edge as usize;
                if line >= text.len_lines() - 1 {
                    return range;
                }
                line_end_char_index(&text, line) - !inclusive as usize
            }
            Direction::Backward => {
                if inclusive {
                    let line = cursor_line as isize - count as isize;
                    if line < 0 {
                        return range;
                    }
                    line_end_char_index(&text, line as usize)
                } else {
                    let on_edge = text.line_to_char(cursor_line) == cursor_anchor;
                    let line = cursor_line as isize - count as isize + 1 - on_edge as isize;
                    if line <= 0 {
                        return range;
                    }
                    text.line_to_char(line as usize)
                }
            }
        };

        if extend {
            range.put_cursor(text, pos, true)
        } else {
            Range::point(range.cursor(text)).put_cursor(text, pos, true)
        }
    });
    doc.set_selection(view.id, selection);
}

fn find_char(cx: &mut Context, direction: Direction, inclusive: bool, extend: bool) {
    // TODO: count is reset to 1 before next key so we move it into the closure here.
    // Would be nice to carry over.
    let count = cx.count();

    // need to wait for next key
    // TODO: should this be done by grapheme rather than char?  For example,
    // we can't properly handle the line-ending CRLF case here in terms of char.
    cx.on_next_key(move |cx, event| {
        let motion: Motion = if event.code == KeyCode::Enter {
            Box::new(move |editor: &mut Editor| {
                find_char_line_ending_motion(editor, count, direction, inclusive, extend);
            })
        } else if let Some(ch) = match event.code {
            KeyCode::Tab => Some('\t'),
            KeyCode::Char(ch) => Some(ch),
            _ => None,
        } {
            Box::new(move |editor: &mut Editor| {
                let (view, doc) = current!(editor);
                let text = doc.text().slice(..);

                let selection = doc.selection(view.id).clone().transform(|range| {
                    let cursor_anchor = range.cursor(text);
                    let cursor_head = next_grapheme_boundary(text, cursor_anchor);

                    // Exclusive search skips the next char after cursor to enable repeated application
                    let search_start_pos = match (inclusive, direction) {
                        (true, Direction::Forward) => cursor_head,
                        (true, Direction::Backward) => cursor_anchor,
                        (false, Direction::Forward) => cursor_head + 1,
                        (false, Direction::Backward) => cursor_anchor.saturating_sub(1),
                    };

                    search::find_nth_char(count, text, ch, search_start_pos, direction)
                        // Exclusive search should stop on previous character
                        .map(|pos| match (inclusive, direction) {
                            (true, Direction::Forward) => pos,
                            (true, Direction::Backward) => pos,
                            (false, Direction::Forward) => pos - 1,
                            (false, Direction::Backward) => pos + 1,
                        })
                        .map_or(range, |pos| {
                            if extend {
                                range.put_cursor(text, pos, true)
                            } else {
                                Range::point(range.cursor(text)).put_cursor(text, pos, true)
                            }
                        })
                });

                doc.set_selection(view.id, selection);
            })
        } else {
            return;
        };

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
