//! State belonging to one document/view relationship.

use crate::view::ViewPosition;
use editor_core::Selection;
use text_layout::text_annotations::Overlay;

#[derive(Debug)]
pub(crate) struct DocumentViewState {
    pub selection: Selection,
    pub(super) folds: super::folding::Folds,
    pub position: ViewPosition,
    pub jump_labels: Vec<Overlay>,
    pub restore_append_cursor: bool,
    pub expansion_history: Vec<Selection>,
    pub(super) selection_undo: super::selection_undo::SelectionUndo,
    /// False while `selection` is a placeholder from `Document::view_state_mut`,
    /// which the view's first real selection replaces without recording it.
    pub(super) selection_shown: bool,
    pub hover: Option<event::TaskController>,
    pub navigation: Option<event::TaskController>,
    pub code_actions: Option<event::TaskController>,
    pub shell_filters: Option<event::TaskController>,
    pub inlay_hints: crate::features::inlay_hints::InlayHintsState,
    pub document_highlights: crate::features::document_highlight::DocumentHighlightsState,
    pub code_action_hints: crate::features::code_action_hint::CodeActionHints,
    pub breadcrumbs: Option<Vec<crate::features::document_symbols::Crumb>>,
    pub search: crate::features::buffer_search::State,
    pub search_highlight: Option<(crate::features::buffer_search::Query, buffer::Snapshot)>,
}

impl DocumentViewState {
    pub(super) fn new(selection: Selection) -> Self {
        Self {
            selection,
            folds: Default::default(),
            position: ViewPosition::default(),
            jump_labels: Vec::new(),
            restore_append_cursor: false,
            expansion_history: Vec::new(),
            selection_undo: Default::default(),
            selection_shown: true,
            hover: Default::default(),
            navigation: Default::default(),
            code_actions: Default::default(),
            shell_filters: Default::default(),
            inlay_hints: Default::default(),
            document_highlights: Default::default(),
            code_action_hints: Default::default(),
            breadcrumbs: None,
            search: Default::default(),
            search_highlight: None,
        }
    }

    /// State created before the view has a selection of its own.
    pub(super) fn placeholder(selection: Selection) -> Self {
        Self {
            selection_shown: false,
            ..Self::new(selection)
        }
    }
}

impl super::Document {
    /// Active search highlighting belongs to a view and expires when its buffer changes.
    pub fn search_highlight(
        &self,
        view: crate::ViewId,
    ) -> Option<&crate::features::buffer_search::Query> {
        let (query, snapshot) = self.view_states.get(&view)?.search_highlight.as_ref()?;
        snapshot.is_current(&self.buffer).then_some(query)
    }

    pub fn set_search_highlight(
        &mut self,
        view: crate::ViewId,
        query: Option<crate::features::buffer_search::Query>,
    ) {
        let snapshot = self.snapshot();
        self.view_state_mut(view).search_highlight = query.map(|query| (query, snapshot));
    }

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

impl super::Document {
    pub(crate) fn cancel_view_request(
        &mut self,
        view: crate::ViewId,
        request: fn(&mut DocumentViewState) -> &mut Option<event::TaskController>,
    ) {
        if let Some(state) = self.view_states.get_mut(&view) {
            *request(state) = None;
        }
    }

    pub(crate) fn cancel_view_requests(
        &mut self,
        request: fn(&mut DocumentViewState) -> &mut Option<event::TaskController>,
    ) {
        for state in self.view_states.values_mut() {
            *request(state) = None;
        }
    }
}
