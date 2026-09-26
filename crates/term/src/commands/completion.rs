//! Explicit completion requests from terminal input.

use crate::commands::context::Context;

pub fn completion(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let range = doc.selection(view.id).primary();
    let text = doc.text().slice(..);
    let cursor = range.cursor(text);

    cx.editor
        .handlers
        .trigger_completions(cursor, doc.id(), view.id);
}
