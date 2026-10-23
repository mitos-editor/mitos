//! Mode-transition commands delegated to the shared editor.

use super::context::Context;
use view::document::Mode;

pub(super) fn insert_mode(cx: &mut Context) {
    cx.editor.enter_insert_mode();
}

pub(super) fn append_mode(cx: &mut Context) {
    if let Err(error) = cx.editor.enter_append_mode() {
        cx.editor.set_error(|| error.to_string());
    }
}

pub(super) fn normal_mode(cx: &mut Context) {
    cx.editor.enter_normal_mode();
}

pub(super) fn debug_mode(cx: &mut Context) {
    cx.editor.enter_debug_mode();
}

pub(super) fn select_mode(cx: &mut Context) {
    cx.editor.enter_select_mode();
}

pub(super) fn exit_select_mode(cx: &mut Context) {
    if cx.editor.mode() == Mode::Select {
        cx.editor.enter_normal_mode();
    }
}
