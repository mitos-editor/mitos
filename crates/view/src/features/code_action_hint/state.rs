//! Code-action availability and request lifetime for one document/view relationship.

use event::TaskController;

use crate::{Document, ViewId};

#[derive(Debug, Default)]
pub(crate) struct CodeActionHints {
    available: bool,
    request: TaskController,
}

impl CodeActionHints {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            available: self.available,
            ..Self::default()
        }
    }
}

impl Document {
    pub(crate) fn set_code_action_hints(&mut self, view_id: ViewId) {
        self.view_state_mut(view_id).code_action_hint.available = true;
    }

    pub(crate) fn clear_code_action_hints(&mut self, view_id: ViewId) {
        if let Some(state) = self.view_states.get_mut(&view_id) {
            state.code_action_hint = Default::default();
        }
    }

    pub(crate) fn clear_all_code_action_hints(&mut self) {
        for state in self.view_states.values_mut() {
            state.code_action_hint = Default::default();
        }
    }

    pub fn code_action_hints(&self, view_id: ViewId) -> bool {
        self.view_states
            .get(&view_id)
            .is_some_and(|state| state.code_action_hint.available)
    }

    pub(crate) fn code_action_controller(&mut self, view_id: ViewId) -> &mut TaskController {
        &mut self.view_state_mut(view_id).code_action_hint.request
    }
}
