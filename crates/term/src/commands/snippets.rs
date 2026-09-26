//! Navigation between snippet placeholders.

use crate::commands::{context::Context, insert::insert_char};
use editor_core::movement::Direction;
use view::document::Mode;

pub(super) fn goto_next_tabstop(cx: &mut Context) {
    goto_next_tabstop_impl(cx, Direction::Forward)
}

pub(super) fn goto_prev_tabstop(cx: &mut Context) {
    goto_next_tabstop_impl(cx, Direction::Backward)
}

fn goto_next_tabstop_impl(cx: &mut Context, direction: Direction) {
    let (view, doc) = current!(cx.editor);
    let view_id = view.id;
    let Some(mut snippet) = doc.active_snippet.take() else {
        cx.editor.set_error(|| "no snippet is currently active");
        return;
    };
    let tabstop = match direction {
        Direction::Forward => Some(snippet.next_tabstop(doc.selection(view_id))),
        Direction::Backward => snippet
            .prev_tabstop(doc.selection(view_id))
            .map(|selection| (selection, false)),
    };
    let Some((selection, last_tabstop)) = tabstop else {
        return;
    };
    doc.set_selection(view_id, selection);
    if !last_tabstop {
        doc.active_snippet = Some(snippet)
    }
    if cx.editor.mode() == Mode::Insert {
        cx.on_next_key_fallback(|cx, key| {
            if let Some(c) = key.char() {
                let (view, doc) = current!(cx.editor);
                if let Some(snippet) = &doc.active_snippet {
                    doc.apply(&snippet.delete_placeholder(doc.text()), view.id);
                }
                insert_char(cx, c);
            }
        })
    }
}
