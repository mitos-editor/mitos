use std::num::NonZeroUsize;

use editor_core::{
    graphemes,
    line_ending::line_end_char_index,
    movement::{Direction, Movement},
};
use view::{document::Mode, editor::Action, Document, Editor, View};

use super::context::Context;

pub(super) fn goto_next_buffer(cx: &mut Context) {
    goto_buffer(cx.editor, Direction::Forward, cx.count());
}

pub(super) fn goto_previous_buffer(cx: &mut Context) {
    goto_buffer(cx.editor, Direction::Backward, cx.count());
}

pub(super) fn goto_buffer(editor: &mut Editor, direction: Direction, count: usize) {
    let current = view!(editor).doc;

    let id = match direction {
        Direction::Forward => {
            let iter = editor.documents.keys();
            // skip 'count' times past current buffer
            iter.cycle().skip_while(|id| *id != &current).nth(count)
        }
        Direction::Backward => {
            let iter = editor.documents.keys();
            // skip 'count' times past current buffer
            iter.rev()
                .cycle()
                .skip_while(|id| *id != &current)
                .nth(count)
        }
    }
    .unwrap();

    let id = *id;

    editor.switch(id, Action::Replace);
}

pub(super) fn goto_file_start(cx: &mut Context) {
    goto_file_start_impl(cx, Movement::Move);
}

pub(super) fn extend_to_file_start(cx: &mut Context) {
    goto_file_start_impl(cx, Movement::Extend);
}

fn goto_file_start_impl(cx: &mut Context, movement: Movement) {
    if cx.count.is_some() {
        goto_line_impl(cx, movement);
    } else {
        let (view, doc) = current!(cx.editor);
        let text = doc.text().slice(..);
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, 0, movement == Movement::Extend));
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
    }
}

pub(super) fn goto_file_end(cx: &mut Context) {
    goto_file_end_impl(cx, Movement::Move);
}

pub(super) fn extend_to_file_end(cx: &mut Context) {
    goto_file_end_impl(cx, Movement::Extend)
}

fn goto_file_end_impl(cx: &mut Context, movement: Movement) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let pos = doc.text().len_chars();
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

// Store a jump on the jumplist.
pub(super) fn push_jump(view: &mut View, doc: &mut Document) {
    doc.append_changes_to_history(view);
    let jump = (doc.id(), doc.selection(view.id).clone());
    view.push_jump(doc, jump);
}

pub(super) fn goto_line(cx: &mut Context) {
    goto_line_impl(cx, Movement::Move);
}

fn goto_line_impl(cx: &mut Context, movement: Movement) {
    if cx.count.is_some() {
        let (view, doc) = current!(cx.editor);
        push_jump(view, doc);

        goto_line_without_jumplist(cx.editor, cx.count, movement);
    }
}

pub(super) fn goto_line_without_jumplist(
    editor: &mut Editor,
    count: Option<NonZeroUsize>,
    movement: Movement,
) {
    if let Some(count) = count {
        let (view, doc) = current!(editor);
        let text = doc.text().slice(..);
        let max_line = if text.line(text.len_lines() - 1).len_chars() == 0 {
            // If the last line is blank, don't jump to it.
            text.len_lines().saturating_sub(2)
        } else {
            text.len_lines() - 1
        };
        let line_idx = std::cmp::min(count.get() - 1, max_line);
        let pos = text.line_to_char(line_idx);
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));

        doc.set_selection(view.id, selection);
    }
}

pub(super) fn goto_last_line(cx: &mut Context) {
    goto_last_line_impl(cx, Movement::Move)
}

pub(super) fn extend_to_last_line(cx: &mut Context) {
    goto_last_line_impl(cx, Movement::Extend)
}

fn goto_last_line_impl(cx: &mut Context, movement: Movement) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let line_idx = if text.line(text.len_lines() - 1).len_chars() == 0 {
        // If the last line is blank, don't jump to it.
        text.len_lines().saturating_sub(2)
    } else {
        text.len_lines() - 1
    };
    let pos = text.line_to_char(line_idx);
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));

    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

pub(super) fn goto_column(cx: &mut Context) {
    goto_column_impl(cx, Movement::Move);
}

pub(super) fn extend_to_column(cx: &mut Context) {
    goto_column_impl(cx, Movement::Extend);
}

fn goto_column_impl(cx: &mut Context, movement: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone().transform(|range| {
        let line = range.cursor_line(text);
        let line_start = text.line_to_char(line);
        let line_end = line_end_char_index(&text, line);
        let pos = graphemes::nth_next_grapheme_boundary(text, line_start, count - 1).min(line_end);
        range.put_cursor(text, pos, movement == Movement::Extend)
    });
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

pub(super) fn goto_last_accessed_file(cx: &mut Context) {
    let view = view_mut!(cx.editor);
    if let Some(alt) = view.docs_access_history.pop() {
        cx.editor.switch(alt, Action::Replace);
    } else {
        cx.editor.set_error(|| "no last accessed buffer")
    }
}

pub(super) fn goto_last_modification(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let pos = doc.history.get_mut().last_edit_pos();
    let text = doc.text().slice(..);
    if let Some(pos) = pos {
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, pos, cx.editor.mode == Mode::Select));
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
    }
}

pub(super) fn goto_last_modified_file(cx: &mut Context) {
    let view = view!(cx.editor);
    let alternate_file = view
        .last_modified_docs
        .into_iter()
        .flatten()
        .find(|&id| id != view.doc);
    if let Some(alt) = alternate_file {
        cx.editor.switch(alt, Action::Replace);
    } else {
        cx.editor.set_error(|| "no last modified buffer")
    }
}

pub(super) fn jump_forward(cx: &mut Context) {
    cx.editor.jump_forward(cx.editor.tree.focus, cx.count());
}

pub(super) fn jump_backward(cx: &mut Context) {
    cx.editor.jump_backward(cx.editor.tree.focus, cx.count());
}

pub(super) fn save_selection(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    push_jump(view, doc);
    cx.editor.set_status("Selection saved to jumplist");
}
