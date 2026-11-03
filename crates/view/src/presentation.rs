//! Concrete presentation state shared by one editor's services and frontend.

use crate::{editor::EditorEvent, features};
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc};

#[derive(Default)]
pub(crate) struct Pending {
    pub completion: VecDeque<features::completion::CompletionUpdate>,
    pub navigation: Option<features::navigation::NavigationUpdate>,
    pub hover: Option<features::hover::HoverUpdate>,
    pub signature_help: Option<features::signature_help::SignatureHelpUpdate>,
    pub reload: VecDeque<features::auto_reload::ReloadRequest>,
    pub trust: VecDeque<features::workspace_trust::TrustRequest>,
}

/// Clones retain the editor destination, including document-only signature triggers.
#[derive(Clone, Default)]
pub struct Inbox(Arc<Mutex<Pending>>);

impl Inbox {
    pub(crate) fn with<R>(&self, read: impl FnOnce(&mut Pending) -> R) -> R {
        read(&mut self.0.lock())
    }

    // Singleton slots coalesce; completion deltas and independent prompts stay FIFO.
    // Blame retains its earlier lazy ready probe, followed by this unchanged priority.
    pub(crate) fn next(&self) -> Option<EditorEvent> {
        self.with(|pending| {
            pending
                .completion
                .pop_front()
                .map(EditorEvent::Completion)
                .or_else(|| pending.navigation.take().map(EditorEvent::Navigation))
                .or_else(|| pending.hover.take().map(EditorEvent::Hover))
                .or_else(|| {
                    pending
                        .signature_help
                        .take()
                        .map(EditorEvent::SignatureHelp)
                })
                .or_else(|| {
                    pending
                        .reload
                        .pop_front()
                        .map(EditorEvent::ReloadConfirmation)
                })
                .or_else(|| pending.trust.pop_front().map(EditorEvent::WorkspaceTrust))
        })
    }
}
