//! Mandatory document invariants established before notifying observers.

use super::Document;
use crate::ViewId;
use editor_core::{ChangeSet, Selection};

impl Document {
    pub(super) fn update_selection(&mut self, view: ViewId, selection: Selection) {
        let selection = selection.ensure_invariants(self.buffer.text().slice(..));
        match self.view_states.entry(view) {
            std::collections::hash_map::Entry::Occupied(mut state) => {
                state.get_mut().selection = selection
            }
            std::collections::hash_map::Entry::Vacant(state) => {
                state.insert(super::DocumentViewState::new(selection));
            }
        }
        if self
            .active_snippet
            .as_ref()
            .is_some_and(|snippet| !snippet.is_valid(self.selection(view)))
        {
            self.active_snippet = None;
        }
    }

    pub(super) fn map_snippet(&mut self, changes: &ChangeSet) {
        if self
            .active_snippet
            .as_mut()
            .is_some_and(|snippet| snippet.map(changes))
        {
            self.active_snippet = None;
        }
    }
}
