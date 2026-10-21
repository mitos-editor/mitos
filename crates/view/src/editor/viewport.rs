//! Viewport operations target an explicit split without changing editor focus.

use crate::{
    editing::movement::{self, ScrollCursor},
    Editor, ViewId,
};
use editor_core::movement::{Direction, Movement};

impl Editor {
    /// Scroll one split, preserving focus, mode, and other splits' state.
    /// Closed splits are ignored, as they may disappear between input and delivery.
    pub fn scroll_view(
        &mut self,
        view_id: ViewId,
        count: usize,
        direction: Direction,
        cursor: ScrollCursor,
        behaviour: Movement,
    ) {
        let padding = self.config().scroll.padding;
        let Some(view) = self.tree.try_get(view_id) else {
            return;
        };
        let doc = self
            .documents
            .get_mut(&view.doc)
            .expect("view document is registered");
        movement::scroll(doc, view, count, direction, cursor, behaviour, padding);
    }
}
