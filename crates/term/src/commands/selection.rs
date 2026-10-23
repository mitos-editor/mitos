//! Selection creation, range transformations, filtering, and primary selection commands.

use editor_core::{
    movement::Direction,
    selection::{self, operations},
    Range, Selection,
};

use super::context::Context;
use crate::ui::{self, PromptEvent};

pub(super) fn trim_selections(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = operations::trim(doc.text().slice(..), doc.selection(view.id));
    doc.set_selection(view.id, selection);
}

fn copy_selection_on_line(cx: &mut Context, direction: Direction) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let selection = operations::copy_on_line(
        doc.text().slice(..),
        doc.selection(view.id),
        doc.tab_width(),
        direction,
        count,
    );
    doc.set_selection(view.id, selection);
}

pub(super) fn copy_selection_on_prev_line(cx: &mut Context) {
    copy_selection_on_line(cx, Direction::Backward)
}

pub(super) fn copy_selection_on_next_line(cx: &mut Context) {
    copy_selection_on_line(cx, Direction::Forward)
}

pub(super) fn select_all(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);

    let end = doc.text().len_chars();
    doc.set_selection(view.id, Selection::single(0, end))
}

pub(super) fn select_regex(cx: &mut Context) {
    let reg = cx.register.unwrap_or('/');
    ui::regex_prompt(
        cx,
        "select:".into(),
        Some(reg),
        ui::completers::none,
        move |cx, regex, event| {
            let (view, doc) = current!(cx.editor);
            if !matches!(event, PromptEvent::Update | PromptEvent::Validate) {
                return;
            }
            let text = doc.text().slice(..);
            match selection::select_on_matches(text, doc.selection(view.id), &regex) {
                Some(selection) => {
                    doc.set_selection(view.id, selection);
                }
                _ => {
                    if event == PromptEvent::Validate {
                        cx.editor.set_error(|| "nothing selected");
                    }
                }
            }
        },
    );
}

pub(super) fn select_character(cx: &mut Context) {
    cx.on_next_key(|cx, event| {
        use view::input::KeyCode;
        let ch = match event.code {
            KeyCode::Char(ch) => ch,
            KeyCode::Enter => '\n',
            KeyCode::Tab => '\t',
            _ => return,
        };
        let mut bytes = [0; 4];
        let regex = stdx::rope::Regex::new(&editor_core::regex::escape(ch.encode_utf8(&mut bytes)))
            .expect("an escaped character is a valid regex");
        let (view, doc) = current!(cx.editor);
        if let Some(selection) =
            selection::select_on_matches(doc.text().slice(..), doc.selection(view.id), &regex)
        {
            doc.set_selection(view.id, selection);
        } else {
            cx.editor.set_error(|| "nothing selected");
        }
    });
}

pub(super) fn split_selection(cx: &mut Context) {
    let reg = cx.register.unwrap_or('/');
    ui::regex_prompt(
        cx,
        "split:".into(),
        Some(reg),
        ui::completers::none,
        move |cx, regex, event| {
            let (view, doc) = current!(cx.editor);
            if !matches!(event, PromptEvent::Update | PromptEvent::Validate) {
                return;
            }
            let text = doc.text().slice(..);
            let selection = selection::split_on_matches(text, doc.selection(view.id), &regex);
            doc.set_selection(view.id, selection);
        },
    );
}

pub(super) fn split_selection_on_newline(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let selection = selection::split_on_newline(text, doc.selection(view.id));
    doc.set_selection(view.id, selection);
}

pub(super) fn merge_selections(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = doc.selection(view.id).clone().merge_ranges();
    doc.set_selection(view.id, selection);
}

pub(super) fn merge_consecutive_selections(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = doc.selection(view.id).clone().merge_consecutive_ranges();
    doc.set_selection(view.id, selection);
}

pub(super) fn extend_line(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let direction = doc.selection(view.id).primary().direction();
    extend_line_impl(cx, direction);
}

pub(super) fn extend_line_below(cx: &mut Context) {
    extend_line_impl(cx, Direction::Forward);
}

pub(super) fn extend_line_above(cx: &mut Context) {
    extend_line_impl(cx, Direction::Backward);
}
fn extend_line_impl(cx: &mut Context, direction: Direction) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let selection = operations::extend_line(
        doc.text().slice(..),
        doc.selection(view.id),
        direction,
        count,
    );
    doc.set_selection(view.id, selection);
}

pub(super) fn select_line_below(cx: &mut Context) {
    select_line_impl(cx, Direction::Forward);
}
pub(super) fn select_line_above(cx: &mut Context) {
    select_line_impl(cx, Direction::Backward);
}
fn select_line_impl(cx: &mut Context, direction: Direction) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let selection = operations::select_line(
        doc.text().slice(..),
        doc.selection(view.id),
        direction,
        count,
    );
    doc.set_selection(view.id, selection);
}

pub(super) fn extend_to_line_bounds(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = operations::extend_to_line_bounds(doc.text().slice(..), doc.selection(view.id));
    doc.set_selection(view.id, selection);
}

pub(super) fn shrink_to_line_bounds(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = operations::shrink_to_line_bounds(doc.text().slice(..), doc.selection(view.id));
    doc.set_selection(view.id, selection);
}

pub(super) fn collapse_selection(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let pos = range.cursor(text);
        Range::new(pos, pos)
    });
    doc.set_selection(view.id, selection);
}

pub(super) fn flip_selections(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);

    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.flip());
    doc.set_selection(view.id, selection);
}

pub(super) fn ensure_selections_forward(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);

    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|r| r.with_direction(Direction::Forward));

    doc.set_selection(view.id, selection);
}

fn keep_or_remove_selections_impl(cx: &mut Context, remove: bool) {
    // keep or remove selections matching regex
    let reg = cx.register.unwrap_or('/');
    ui::regex_prompt(
        cx,
        if remove { "remove:" } else { "keep:" }.into(),
        Some(reg),
        ui::completers::none,
        move |cx, regex, event| {
            let (view, doc) = current!(cx.editor);
            if !matches!(event, PromptEvent::Update | PromptEvent::Validate) {
                return;
            }
            let text = doc.text().slice(..);

            match selection::keep_or_remove_matches(text, doc.selection(view.id), &regex, remove) {
                Some(selection) => {
                    doc.set_selection(view.id, selection);
                }
                _ => {
                    if event == PromptEvent::Validate {
                        cx.editor.set_error(|| "no selections remaining");
                    }
                }
            }
        },
    )
}

pub(super) fn keep_selections(cx: &mut Context) {
    keep_or_remove_selections_impl(cx, false)
}

pub(super) fn remove_selections(cx: &mut Context) {
    keep_or_remove_selections_impl(cx, true)
}

pub(super) fn keep_primary_selection(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    // TODO: handle count

    let range = doc.selection(view.id).primary();
    doc.set_selection(view.id, Selection::single(range.anchor, range.head));
}

pub(super) fn remove_primary_selection(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    // TODO: handle count

    let selection = doc.selection(view.id);
    if selection.len() == 1 {
        cx.editor.set_error(|| "no selections remaining");
        return;
    }
    let index = selection.primary_index();
    let selection = selection.clone().remove(index);

    doc.set_selection(view.id, selection);
}

fn rotate_selections(cx: &mut Context, direction: Direction) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let selection = editor_core::selection::operations::rotate_primary(
        doc.selection(view.id),
        direction,
        count,
    );
    doc.set_selection(view.id, selection);
}
pub(super) fn rotate_selections_forward(cx: &mut Context) {
    rotate_selections(cx, Direction::Forward)
}
pub(super) fn rotate_selections_backward(cx: &mut Context) {
    rotate_selections(cx, Direction::Backward)
}

pub(super) fn rotate_selections_first(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let mut selection = doc.selection(view.id).clone();
    selection.set_primary_index(0);
    doc.set_selection(view.id, selection);
}

pub(super) fn rotate_selections_last(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let mut selection = doc.selection(view.id).clone();
    let len = selection.len();
    selection.set_primary_index(len - 1);
    doc.set_selection(view.id, selection);
}
