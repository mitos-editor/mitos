//! Per-view code-action availability and request cancellation.

use event::TaskController;
use std::collections::HashMap;

use crate::Document;
use crate::{features::code_action_hint::CodeActionHintHandler, ViewId};

#[derive(Default)]
pub(crate) struct CodeActionHints {
    views: HashMap<ViewId, (bool, Option<TaskController>)>,
    pub(super) handler: Option<CodeActionHintHandler>,
}

impl CodeActionHints {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            views: self
                .views
                .iter()
                .filter(|(_, (available, _))| *available)
                .map(|(&view, _)| (view, (true, None)))
                .collect(),
            ..Self::default()
        }
    }

    pub(crate) fn bind_handler(&mut self, handler: CodeActionHintHandler) {
        self.handler = Some(handler);
    }

    pub(crate) fn remove_view(&mut self, view: ViewId) {
        self.views.remove(&view);
    }

    fn clear(&mut self) {
        self.views.clear();
    }
}

impl Document {
    pub(crate) fn set_code_action_hints(&mut self, view_id: ViewId) {
        self.code_action_hints.views.entry(view_id).or_default().0 = true;
    }

    pub(crate) fn clear_code_action_hints(&mut self, view_id: ViewId) {
        if let Some((available, controller)) = self.code_action_hints.views.get_mut(&view_id) {
            *available = false;
            *controller = None;
        }
    }

    pub(crate) fn clear_all_code_action_hints(&mut self) {
        self.code_action_hints.clear();
    }

    pub fn code_action_hints(&self, view_id: ViewId) -> bool {
        self.code_action_hints
            .views
            .get(&view_id)
            .is_some_and(|(available, _)| *available)
    }

    pub(crate) fn code_action_controller(&mut self, view_id: ViewId) -> &mut TaskController {
        self.code_action_hints
            .views
            .entry(view_id)
            .or_default()
            .1
            .get_or_insert_default()
    }
}
