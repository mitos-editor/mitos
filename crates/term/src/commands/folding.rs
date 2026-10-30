//! Terminal bindings for document/view folding operations.

use super::Context;
use view::document::FoldAction;

fn fold(cx: &mut Context, action: FoldAction) {
    let count = match action {
        FoldAction::Close | FoldAction::Open | FoldAction::Toggle => cx.count(),
        _ => 1,
    };
    if matches!(
        action,
        FoldAction::Close | FoldAction::Toggle | FoldAction::CloseAll
    ) {
        let doc = doc!(cx.editor).id();
        cx.editor.ensure_syntax(doc);
    }
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        doc.fold(view.id, action);
    }
}

pub(super) fn fold_close(cx: &mut Context) {
    fold(cx, FoldAction::Close);
}
pub(super) fn fold_open(cx: &mut Context) {
    fold(cx, FoldAction::Open);
}
pub(super) fn fold_toggle(cx: &mut Context) {
    fold(cx, FoldAction::Toggle);
}
pub(super) fn fold_selection(cx: &mut Context) {
    fold(cx, FoldAction::Selection);
}
pub(super) fn fold_all(cx: &mut Context) {
    fold(cx, FoldAction::CloseAll);
}
pub(super) fn unfold_all(cx: &mut Context) {
    fold(cx, FoldAction::OpenAll);
}
