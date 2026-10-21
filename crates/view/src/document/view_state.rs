//! State belonging to one document/view relationship.

use crate::view::ViewPosition;
use editor_core::Selection;
use text_layout::text_annotations::Overlay;

#[derive(Debug, Clone)]
pub(super) struct DocumentViewState {
    pub selection: Selection,
    pub position: ViewPosition,
    pub jump_labels: Vec<Overlay>,
    pub restore_append_cursor: bool,
    pub expansion_history: Vec<Selection>,
}

impl DocumentViewState {
    pub(super) fn new(selection: Selection) -> Self {
        Self {
            selection,
            position: ViewPosition::default(),
            jump_labels: Vec::new(),
            restore_append_cursor: false,
            expansion_history: Vec::new(),
        }
    }
}

impl super::Document {
    pub(crate) fn set_append_cursor_restoration(&mut self, view: crate::ViewId, restore: bool) {
        self.view_state_mut(view).restore_append_cursor = restore;
    }

    pub(crate) fn take_append_cursor_restoration(&mut self, view: crate::ViewId) -> bool {
        self.view_states
            .get_mut(&view)
            .is_some_and(|state| std::mem::take(&mut state.restore_append_cursor))
    }

    pub(crate) fn take_expansion_history(&mut self, view: crate::ViewId) -> Vec<Selection> {
        self.view_states
            .get_mut(&view)
            .map(|state| std::mem::take(&mut state.expansion_history))
            .unwrap_or_default()
    }

    pub(crate) fn set_expanded_selection(
        &mut self,
        view: crate::ViewId,
        selection: Selection,
        history: Vec<Selection>,
    ) {
        self.update_selection(view, selection);
        self.view_state_mut(view).expansion_history = history;
        self.selection_did_change(view);
    }
}
