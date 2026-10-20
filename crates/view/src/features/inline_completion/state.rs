//! Inline completion cache and document/view lifetime.
use super::requests::InlineCompletionTrigger;
use crate::ViewId;
use editor_core::{movement::Direction, Range};
use event::TaskController;
use lsp_client::{lsp, LanguageServerId};

pub struct InlineCompletion {
    pub cursor: usize,
    pub range: Range,
    pub text: String,
    /// Preview includes the unchanged suffix after the replacement range.
    pub lines: Vec<String>,
    pub(crate) server: LanguageServerId,
    pub(crate) command: Option<lsp::Command>,
}

#[derive(Default)]
pub(crate) struct InlineCompletions {
    pub(super) items: Vec<InlineCompletion>,
    pub(super) index: usize,
    pub(super) view: Option<ViewId>,
    pub(super) controller: TaskController,
    pub(super) trigger: Option<InlineCompletionTrigger>,
}

impl InlineCompletions {
    pub(crate) fn bind_handler(&mut self, handler: &super::InlineCompletionHandler) {
        self.trigger = Some(handler.trigger.clone());
    }

    pub fn current(&self, view: ViewId) -> Option<&InlineCompletion> {
        (self.view == Some(view))
            .then(|| self.items.get(self.index))
            .flatten()
    }

    pub fn clear(&mut self) {
        self.controller.cancel();
        self.items.clear();
        self.index = 0;
        self.view = None;
    }

    pub(crate) fn cycle(&mut self, view: ViewId, direction: Direction) {
        if self.view != Some(view) {
            return;
        }
        if !self.items.is_empty() {
            self.index = match direction {
                Direction::Forward => (self.index + 1) % self.items.len(),
                Direction::Backward => (self.index + self.items.len() - 1) % self.items.len(),
            };
        }
    }
}

impl InlineCompletions {
    pub(crate) fn remove_view(&mut self, doc: crate::DocumentId, view: ViewId) {
        if self.view == Some(view) {
            self.clear();
        }
        if let Some(trigger) = &self.trigger {
            trigger.cancel_view(doc, view);
        }
    }
}

impl crate::Document {
    pub fn inline_completion(&self, view: ViewId) -> Option<&InlineCompletion> {
        self.inline_completions.current(view)
    }
}
