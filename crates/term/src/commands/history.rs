//! Edit-history commands: count adaptation, history limits, and explicit checkpoints.

use editor_core::history::UndoKind;

use super::context::Context;

pub(super) fn undo(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        if !doc.undo(view) {
            cx.editor.set_status("Already at oldest change");
            break;
        }
    }
}

pub(super) fn redo(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        if !doc.redo(view) {
            cx.editor.set_status("Already at newest change");
            break;
        }
    }
}

pub(super) fn earlier(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        // rather than doing in batch we do this so get error halfway
        if !doc.earlier(view, UndoKind::Steps(1)) {
            cx.editor.set_status("Already at oldest change");
            break;
        }
    }
}

pub(super) fn later(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        // rather than doing in batch we do this so get error halfway
        if !doc.later(view, UndoKind::Steps(1)) {
            cx.editor.set_status("Already at newest change");
            break;
        }
    }
}

pub(super) fn commit_undo_checkpoint(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    doc.append_changes_to_history(view);
}
