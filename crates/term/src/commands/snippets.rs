//! Frontend messages for shared snippet navigation.
use crate::commands::context::Context;
use editor_core::movement::Direction;
use view::editor::SnippetNavigation;

pub(super) fn goto_next_tabstop(cx: &mut Context) {
    navigate(cx, Direction::Forward);
}
pub(super) fn goto_prev_tabstop(cx: &mut Context) {
    navigate(cx, Direction::Backward);
}
fn navigate(cx: &mut Context, direction: Direction) {
    if cx.editor.move_snippet_tabstop(direction) == SnippetNavigation::Inactive {
        cx.editor.set_error(|| "no snippet is currently active");
    }
}
