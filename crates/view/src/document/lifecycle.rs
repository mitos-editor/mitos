//! Mandatory document invariants established before notifying observers.

use super::Document;
use crate::ViewId;
use editor_core::{ChangeSet, Selection};

impl Document {
    /// Cancel language-dependent work before installing a new language configuration.
    pub(super) fn invalidate_language_features(&mut self) {
        self.hover.clear();
        self.navigation.clear();
        self.rename.cancel();
        self.code_actions.clear();
        self.workspace_symbols.cancel();
        self.inlay_hints.clear();
        self.inline_completions.clear();
        self.accepted_completions.cancel();
        self.document_colors.invalidate();
        self.document_links.invalidate();
        self.code_lenses.clear();
        self.document_symbols.clear();
        self.clear_all_document_highlights();
        self.clear_all_code_action_hints();
        self.pull_diagnostics.clear();
    }

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
