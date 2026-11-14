//! Terminal bindings for document/view folding operations.

use super::Context;
use view::document::FoldAction;

fn fold(cx: &mut Context, action: FoldAction) {
    let count = match action {
        FoldAction::Close | FoldAction::Open | FoldAction::Toggle => cx.count(),
        _ => 1,
    };
    let view = view!(cx.editor).id;
    cx.editor.fold(view, action, count);
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

pub(super) fn fold_toggle_all(cx: &mut Context) {
    fold(cx, FoldAction::ToggleAll);
}
