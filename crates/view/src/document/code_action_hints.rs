//! Per-view code-action availability and request cancellation.

use event::TaskController;
use std::collections::{HashMap, HashSet};

use super::Document;
use crate::{handlers::code_action_hint::CodeActionHintHandler, ViewId};

#[derive(Default)]
pub(crate) struct CodeActionHints {
    cache: HashSet<ViewId>,
    requests: HashMap<ViewId, TaskController>,
    pub(crate) handler: Option<CodeActionHintHandler>,
}

impl CodeActionHints {
    pub(super) fn remove_view(&mut self, view: ViewId) {
        self.cache.remove(&view);
        self.requests.remove(&view);
    }

    fn clear(&mut self) {
        self.cache.clear();
        self.requests.clear();
    }
}

impl Document {
    pub(crate) fn set_code_action_hints(&mut self, view_id: ViewId) {
        self.code_action_hints.cache.insert(view_id);
    }

    pub(crate) fn clear_code_action_hints(&mut self, view_id: ViewId) {
        self.code_action_hints.cache.remove(&view_id);
        if let Some(controller) = self.code_action_hints.requests.get_mut(&view_id) {
            controller.cancel();
        }
    }

    pub(crate) fn clear_all_code_action_hints(&mut self) {
        self.code_action_hints.clear();
    }

    pub fn code_action_hints(&self, view_id: ViewId) -> bool {
        self.code_action_hints.cache.contains(&view_id)
    }

    pub(crate) fn code_action_controller(&mut self, view_id: ViewId) -> &mut TaskController {
        self.code_action_hints.requests.entry(view_id).or_default()
    }
}
