//! Per-document/view cache ownership, freshness, cancellation, and edit mapping.

use std::fmt;

use editor_core::{ChangeSet, Uri};
use event::{TaskController, TaskHandle};
use lsp_client::LanguageServerId;
use text_layout::text_annotations::InlineAnnotation;

/// Annotations for one document/view. Separate layers allow styling by hint kind
/// and keep padding before and after the labels in rendering order.
#[derive(Debug, Clone)]
pub struct DocumentInlayHints {
    /// Buffered viewport covered by these annotations.
    pub id: DocumentInlayHintsId,

    /// Inlay hints of `TYPE` kind, if any.
    pub type_inlay_hints: Vec<InlineAnnotation>,

    /// Inlay hints of `PARAMETER` kind, if any.
    pub parameter_inlay_hints: Vec<InlineAnnotation>,

    /// Hints with no kind or an unrecognized kind.
    pub other_inlay_hints: Vec<InlineAnnotation>,

    /// Padding is rendered in separate layers before and after all labels.
    pub padding_before_inlay_hints: Vec<InlineAnnotation>,
    pub padding_after_inlay_hints: Vec<InlineAnnotation>,
}

impl DocumentInlayHints {
    /// Generate an empty list of inlay hints with the given ID.
    pub fn empty_with_id(id: DocumentInlayHintsId) -> Self {
        Self {
            id,
            type_inlay_hints: Vec::new(),
            parameter_inlay_hints: Vec::new(),
            other_inlay_hints: Vec::new(),
            padding_before_inlay_hints: Vec::new(),
            padding_after_inlay_hints: Vec::new(),
        }
    }
}

/// The line range covered by a view's inlay hints. Full cache freshness also depends
/// on the document snapshot and language-server instance retained by the coordinator.
///
/// Line ranges capture coverage changes after soft wrapping or viewport resizing,
/// even when the scroll offset is unchanged.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct DocumentInlayHintsId {
    /// First line for which the inlay hints were requested.
    pub first_line: usize,
    /// Last line for which the inlay hints were requested.
    pub last_line: usize,
}

impl fmt::Debug for DocumentInlayHintsId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentInlayHintsId")
            .field("lines", &(self.first_line..self.last_line))
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(super) struct CacheKey {
    pub(super) version: i32,
    pub(super) uri: Option<Uri>,
    pub(super) server: LanguageServerId,
    pub(super) viewport: DocumentInlayHintsId,
}

#[derive(Debug, Default)]
enum RequestState {
    #[default]
    Idle,
    Pending {
        key: CacheKey,
        controller: TaskController,
    },
    Completed {
        key: CacheKey,
    },
}

impl RequestState {
    fn invalidate(&mut self) {
        // Dropping a pending controller cancels its task handles.
        *self = Self::Idle;
    }
}

/// Cached annotations and request lifetime for one document/view.
#[derive(Debug, Default)]
pub(crate) struct InlayHintsState {
    cache: Option<DocumentInlayHints>,
    request: RequestState,
}

impl InlayHintsState {
    pub(crate) fn get(&self) -> Option<&DocumentInlayHints> {
        self.cache.as_ref()
    }

    pub(crate) fn set(&mut self, hints: DocumentInlayHints) {
        self.cache = Some(hints);
    }

    pub(super) fn is_current(&self, key: &CacheKey) -> bool {
        match &self.request {
            RequestState::Idle => false,
            RequestState::Pending {
                key: current,
                controller,
            } => current == key && controller.is_running(),
            RequestState::Completed { key: current } => current == key,
        }
    }

    pub(super) fn start(&mut self, key: CacheKey) -> TaskHandle {
        let mut controller = TaskController::new();
        let handle = controller.restart();
        self.request = RequestState::Pending { key, controller };
        handle
    }

    pub(super) fn complete(&mut self, hints: DocumentInlayHints) {
        self.cache = Some(hints);
        if let RequestState::Pending { key, .. } = std::mem::take(&mut self.request) {
            self.request = RequestState::Completed { key };
        }
    }

    pub(super) fn retry(&mut self) {
        self.request.invalidate();
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        self.request.invalidate();
        if let Some(hints) = &mut self.cache {
            hints.apply_changes(changes);
        }
    }
}

impl DocumentInlayHints {
    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        for annotations in [
            &mut self.padding_before_inlay_hints,
            &mut self.type_inlay_hints,
            &mut self.parameter_inlay_hints,
            &mut self.other_inlay_hints,
            &mut self.padding_after_inlay_hints,
        ] {
            changes.update_positions(
                annotations
                    .iter_mut()
                    .map(|annotation| (&mut annotation.char_idx, editor_core::Assoc::After)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::Key as _;

    #[test]
    fn failed_and_canceled_requests_can_retry_without_losing_cached_hints() {
        let key = CacheKey {
            version: 1,
            uri: None,
            server: LanguageServerId::null(),
            viewport: DocumentInlayHintsId {
                first_line: 0,
                last_line: 10,
            },
        };
        let mut state = InlayHintsState::default();
        let first = state.start(key.clone());
        assert!(state.is_current(&key));
        drop(first);
        assert!(!state.is_current(&key));

        let second = state.start(key.clone());
        state.complete(DocumentInlayHints::empty_with_id(key.viewport));
        assert!(second.is_canceled());
        assert!(state.is_current(&key));
        state.retry();
        assert!(!state.is_current(&key));
        assert!(state.get().is_some());

        let third = state.start(key.clone());
        state.retry();
        assert!(third.is_canceled());
        assert!(!state.is_current(&key));
        assert!(state.get().is_some());
    }
}
