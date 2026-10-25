//! State belonging to one document/view relationship.

use crate::view::ViewPosition;
use editor_core::Selection;
use text_layout::text_annotations::Overlay;

#[derive(Debug)]
pub(crate) struct DocumentViewState {
    pub selection: Selection,
    pub position: ViewPosition,
    pub jump_labels: Vec<Overlay>,
    pub restore_append_cursor: bool,
    pub expansion_history: Vec<Selection>,
    pub search: crate::features::buffer_search::State,
    pub highlights: crate::features::document_highlight::DocumentHighlightsState,
    pub code_action_hint: crate::features::code_action_hint::CodeActionHints,
    pub inlay_hints: crate::features::inlay_hints::InlayHintsState,
    pub hover: event::TaskController,
    pub code_actions: event::TaskController,
    pub shell_filter: event::TaskController,
    pub breadcrumbs: Option<crate::features::document_symbols::Breadcrumbs>,
}

impl Clone for DocumentViewState {
    fn clone(&self) -> Self {
        Self {
            selection: self.selection.clone(),
            position: self.position,
            jump_labels: self.jump_labels.clone(),
            restore_append_cursor: self.restore_append_cursor,
            expansion_history: self.expansion_history.clone(),
            search: self.search.presentation(),
            highlights: self.highlights.presentation(),
            code_action_hint: self.code_action_hint.presentation(),
            inlay_hints: self.inlay_hints.presentation(),
            hover: Default::default(),
            code_actions: Default::default(),
            shell_filter: Default::default(),
            breadcrumbs: self.breadcrumbs.clone(),
        }
    }
}

impl DocumentViewState {
    pub(super) fn new(selection: Selection) -> Self {
        Self {
            selection,
            position: ViewPosition::default(),
            jump_labels: Vec::new(),
            restore_append_cursor: false,
            expansion_history: Vec::new(),
            search: Default::default(),
            highlights: Default::default(),
            code_action_hint: Default::default(),
            inlay_hints: Default::default(),
            hover: Default::default(),
            code_actions: Default::default(),
            shell_filter: Default::default(),
            breadcrumbs: None,
        }
    }
}

impl super::Document {
    pub(crate) fn search_state_mut(
        &mut self,
        view: crate::ViewId,
    ) -> Option<&mut crate::features::buffer_search::State> {
        Some(&mut self.view_states.get_mut(&view)?.search)
    }

    pub(crate) fn search_cache_is_current(&self, view: crate::ViewId) -> bool {
        self.view_states
            .get(&view)
            .is_some_and(|state| state.search.is_current(&self.buffer))
    }

    /// Last search position for this view; drawing never starts a search.
    pub fn search_position(
        &self,
        view: crate::ViewId,
    ) -> Option<editor_core::search::statistics::Position> {
        self.search_cache_is_current(view).then_some(())?;
        self.view_states.get(&view)?.search.position()
    }

    pub(crate) fn invalidate_search_counts(&mut self) {
        for state in self.view_states.values_mut() {
            state.search = Default::default();
        }
    }

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
