//! Mode-transition commands and their selection adjustments.

use editor_core::{graphemes, Range, Transaction};
use view::document::Mode;

use super::context::Context;

pub(super) fn enter_insert_mode(cx: &mut Context) {
    cx.editor.mode = Mode::Insert;
}

// inserts at the start of each selection
pub(super) fn insert_mode(cx: &mut Context) {
    enter_insert_mode(cx);
    let (view, doc) = current!(cx.editor);

    log::trace!(
        "entering insert mode with sel: {:?}, text: {:?}",
        doc.selection(view.id),
        doc.text().to_string()
    );

    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| Range::new(range.to(), range.from()));

    doc.set_selection(view.id, selection);
}

// inserts at the end of each selection
pub(super) fn append_mode(cx: &mut Context) {
    enter_insert_mode(cx);
    let (view, doc) = current!(cx.editor);
    doc.restore_cursor = true;
    let text = doc.text().slice(..);

    // Make sure there's room at the end of the document if the last
    // selection butts up against it.
    let end = text.len_chars();
    let last_range = doc
        .selection(view.id)
        .iter()
        .last()
        .expect("selection should always have at least one range");
    if !last_range.is_empty() && last_range.to() == end {
        let transaction = Transaction::change(
            doc.text(),
            [(end, end, Some(doc.line_ending.as_str().into()))].into_iter(),
        );
        doc.apply(&transaction, view.id);
    }

    let selection = doc.selection(view.id).clone().transform(|range| {
        Range::new(
            range.from(),
            graphemes::next_grapheme_boundary(doc.text().slice(..), range.to()),
        )
    });
    doc.set_selection(view.id, selection);
}

pub(super) fn normal_mode(cx: &mut Context) {
    cx.editor.enter_normal_mode();
}

pub(super) fn select_mode(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    // Make sure end-of-document selections are also 1-width.
    // (With the exception of being in an empty document, of course.)
    let selection = doc.selection(view.id).clone().transform(|range| {
        if range.is_empty() && range.head == text.len_chars() {
            Range::new(
                graphemes::prev_grapheme_boundary(text, range.anchor),
                range.head,
            )
        } else {
            range
        }
    });
    doc.set_selection(view.id, selection);

    cx.editor.mode = Mode::Select;
}

pub(super) fn exit_select_mode(cx: &mut Context) {
    if cx.editor.mode == Mode::Select {
        cx.editor.mode = Mode::Normal;
    }
}
