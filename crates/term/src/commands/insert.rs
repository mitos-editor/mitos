//! Character and line insertion, indentation, comment continuation, and insert-mode deletion.

use super::{
    context::Context, mode::insert_mode, snippets::goto_next_tabstop, syntax::move_parent_node_end,
};
use crate::key;
use editor_config::SmartTabConfig;
use insert::LineBoundary;
pub use insert::{CommentContinuation, Open};
use stdx::rope::RopeExt as _;
use ui_core::{input::KeyEvent, keyboard::KeyCode};
use view::{document::Mode, editing::insert};

pub fn insert_char(cx: &mut Context, c: char) {
    if let Err(error) = try_insert_char(cx, c) {
        cx.editor.set_error(|| error.to_string());
    }
}

pub(crate) fn try_insert_char(cx: &mut Context, c: char) -> Result<(), view::editing::EditError> {
    cx.editor.insert_char(c)?;
    cx.flush_completion_updates();
    Ok(())
}

pub fn smart_tab(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let view_id = view.id;

    if matches!(
        cx.editor.config().indent.smart_tab,
        Some(SmartTabConfig { enable: true, .. })
    ) {
        let cursors_after_whitespace = doc.selection(view_id).ranges().iter().all(|range| {
            let cursor = range.cursor(doc.text().slice(..));
            let current_line_num = doc.text().char_to_line_idx(cursor);
            let current_line_start = doc.text().line_to_char_idx(current_line_num);
            let left = doc.text().slice_chars(current_line_start..cursor);
            left.chars().all(|c| c.is_whitespace())
        });

        if !cursors_after_whitespace {
            if doc.active_snippet().is_some() {
                goto_next_tabstop(cx);
            } else {
                move_parent_node_end(cx);
            }
            return;
        }
    }

    insert_tab(cx);
}

pub fn insert_tab(cx: &mut Context) {
    insert_tab_impl(cx, 1)
}

fn insert_tab_impl(cx: &mut Context, count: usize) {
    if let Err(error) = cx.editor.insert_tab(count) {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub fn append_char_interactive(cx: &mut Context) {
    // Save the current mode, so we can restore it later.
    let mode = cx.editor.mode();
    if let Err(error) = cx.editor.enter_append_mode() {
        cx.editor.set_error(|| error.to_string());
        return;
    }
    insert_selection_interactive(cx, mode);
}

pub fn insert_char_interactive(cx: &mut Context) {
    let mode = cx.editor.mode();
    insert_mode(cx);
    insert_selection_interactive(cx, mode);
}

fn insert_selection_interactive(cx: &mut Context, old_mode: Mode) {
    let count = cx.count();

    // need to wait for next key
    cx.on_next_key(move |cx, event| {
        match event {
            KeyEvent {
                code: KeyCode::Char(ch),
                ..
            } => {
                for _ in 0..count {
                    if let Err(error) = try_insert_char(cx, ch) {
                        cx.editor.set_error(|| error.to_string());
                        break;
                    }
                }
            }
            key!(Enter) => {
                if count != 1 {
                    cx.editor
                        .set_error(|| "inserting multiple newlines not yet supported");
                    return;
                }
                insert_newline(cx)
            }
            key!(Tab) => insert_tab_impl(cx, count),
            _ => (),
        };
        // Restore the old mode.
        cx.editor.set_mode(old_mode);
    });
}

pub fn insert_newline(cx: &mut Context) {
    if let Err(error) = cx.editor.insert_newline() {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub fn delete_char_backward(cx: &mut Context) {
    let count = cx.count();
    if let Err(error) = cx.editor.delete_char_backward(count) {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub fn delete_char_forward(cx: &mut Context) {
    let count = cx.count();
    if let Err(error) = cx.editor.delete_char_forward(count) {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub fn delete_word_backward(cx: &mut Context) {
    let count = cx.count();
    if let Err(error) = cx.editor.delete_word_backward(count) {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub fn delete_sub_word_backward(cx: &mut Context) {
    let count = cx.count();
    if let Err(error) = cx.editor.delete_sub_word_backward(count) {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub fn delete_word_forward(cx: &mut Context) {
    let count = cx.count();
    if let Err(error) = cx.editor.delete_word_forward(count) {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub fn delete_sub_word_forward(cx: &mut Context) {
    let count = cx.count();
    if let Err(error) = cx.editor.delete_sub_word_forward(count) {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub(super) fn kill_to_line_start(cx: &mut Context) {
    if let Err(error) = cx.editor.kill_to_line_start() {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

pub(super) fn kill_to_line_end(cx: &mut Context) {
    if let Err(error) = cx.editor.kill_to_line_end() {
        cx.editor.set_error(|| error.to_string());
    }
    cx.flush_completion_updates();
}

// `I` inserts at the first nonwhitespace character of each line with a selection.
// If the line is empty, automatically indent.
pub(super) fn insert_at_line_start(cx: &mut Context) {
    insert_with_indent(cx, LineBoundary::LineStart);
}

// `A` inserts at the end of each line with a selection.
// If the line is empty, automatically indent.
pub(super) fn insert_at_line_end(cx: &mut Context) {
    insert_with_indent(cx, LineBoundary::LineEnd);
}

// Enter insert mode and auto-indent the current line if it is empty.
// If the line is not empty, move the cursor to the specified fallback position.
fn insert_with_indent(cx: &mut Context, cursor_fallback: LineBoundary) {
    if let Err(error) = cx.editor.insert_at_line_boundary(cursor_fallback) {
        cx.editor.set_error(|| error.to_string());
    }
}

pub(super) fn open(cx: &mut Context, open: Open, comment_continuation: CommentContinuation) {
    let count = cx.count();
    if let Err(error) = cx.editor.open_lines(open, comment_continuation, count) {
        cx.editor.set_error(|| error.to_string());
    }
}

// o inserts a new line after each line with a selection
pub(super) fn open_below(cx: &mut Context) {
    open(cx, Open::Below, CommentContinuation::Enabled)
}

// O inserts a new line before each line with a selection
pub(super) fn open_above(cx: &mut Context) {
    open(cx, Open::Above, CommentContinuation::Enabled)
}

pub(super) fn add_newline_above(cx: &mut Context) {
    add_newline_impl(cx, Open::Above);
}

pub(super) fn add_newline_below(cx: &mut Context) {
    add_newline_impl(cx, Open::Below)
}

fn add_newline_impl(cx: &mut Context, open: Open) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    if let Err(error) = insert::add_newlines(doc, view.id, open, count) {
        cx.editor.set_error(|| error.to_string());
    }
}
