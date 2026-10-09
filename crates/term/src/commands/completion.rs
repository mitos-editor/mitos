//! Explicit completion requests from terminal input.

use crate::commands::context::Context;

pub fn completion(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let range = doc.selection(view.id).primary();
    let text = doc.text().slice(..);
    let cursor = range.cursor(text);

    cx.editor
        .handlers()
        .trigger_completions(cursor, doc.id(), view.id);
}

pub fn inline_completion_accept(cx: &mut Context) {
    if let Some(applied) = view::handlers::inline_completion::accept(cx.editor) {
        cx.callback.push(Box::new(move |compositor, _| {
            compositor
                .find::<crate::ui::EditorView>()
                .unwrap()
                .record_insert_event(crate::ui::editor::InsertEvent::InlineCompletionApply(
                    applied,
                ));
        }));
    }
}

pub fn inline_completion_dismiss(cx: &mut Context) {
    view::handlers::inline_completion::dismiss(cx.editor);
}

pub fn inline_completion_trigger(cx: &mut Context) {
    view::handlers::inline_completion::trigger(
        cx.editor,
        lsp_client::lsp::InlineCompletionTriggerKind::Invoked,
    );
}

pub fn inline_completion_next(cx: &mut Context) {
    view::handlers::inline_completion::cycle(cx.editor, editor_core::movement::Direction::Forward);
}

pub fn inline_completion_prev(cx: &mut Context) {
    view::handlers::inline_completion::cycle(cx.editor, editor_core::movement::Direction::Backward);
}
