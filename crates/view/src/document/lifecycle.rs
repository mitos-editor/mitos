//! Mandatory document invariants established before notifying observers.

use super::Document;
use crate::ViewId;
use editor_core::Selection;

impl Document {
    /// Cancel language-dependent work before installing a new language configuration.
    pub(super) fn invalidate_language_features(&mut self) {
        for state in self.view_states.values_mut() {
            state.expansion_history.clear();
        }
        self.cancel_hover_requests(None);
        self.navigation.clear();
        self.rename.cancel();
        self.cancel_code_action_requests(None);
        self.workspace_symbols.cancel();
        self.reset_all_inlay_hints();
        self.inline_completions.clear();
        self.accepted_completions.cancel();
        self.document_colors.invalidate();
        self.document_links.invalidate();
        self.code_lenses.clear();
        self.invalidate_document_symbols();
        self.clear_all_document_highlights();
        self.clear_all_code_action_hints();
        self.pull_diagnostics.clear();
    }

    pub(super) fn update_selection(&mut self, view: ViewId, selection: Selection) {
        let selection = selection.ensure_invariants(self.buffer.text().slice(..));
        let changed = self
            .try_selection(view)
            .is_none_or(|current| current != &selection);
        match self.view_states.entry(view) {
            std::collections::hash_map::Entry::Occupied(mut state) => {
                let state = state.get_mut();
                state.search.clear_position();
                if state.selection != selection {
                    state.expansion_history.clear();
                }
                state.selection = selection
            }
            std::collections::hash_map::Entry::Vacant(state) => {
                state.insert(super::DocumentViewState::new(selection));
            }
        }
        self.validate_snippet_selection(view, changed);
    }
}
