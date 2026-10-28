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
struct RequestState {
    controller: TaskController,
    key: Option<CacheKey>,
    completed: bool,
}

impl RequestState {
    fn invalidate(&mut self) {
        self.controller.cancel();
        self.key = None;
        self.completed = false;
    }
}

/// Cached annotations and their request lifetime for one document/view relationship.
#[derive(Debug, Default)]
pub(crate) struct InlayHintsState {
    cache: Option<DocumentInlayHints>,
    request: RequestState,
}

impl InlayHintsState {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn get(&self) -> Option<&DocumentInlayHints> {
        self.cache.as_ref()
    }

    pub(crate) fn set(&mut self, hints: DocumentInlayHints) {
        self.cache = Some(hints);
    }

    pub(super) fn is_current(&self, key: &CacheKey) -> bool {
        self.request.key.as_ref() == Some(key)
            && (self.request.completed || self.request.controller.is_running())
    }

    pub(super) fn start(&mut self, key: CacheKey) -> TaskHandle {
        self.request.key = Some(key);
        self.request.completed = false;
        self.request.controller.restart()
    }

    pub(super) fn complete(&mut self, hints: DocumentInlayHints) {
        self.cache = Some(hints);
        self.request.completed = true;
    }

    pub(super) fn retry(&mut self) {
        self.request.invalidate();
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        self.request.invalidate();
        if let Some(hints) = &mut self.cache {
            for annotations in [
                &mut hints.padding_before_inlay_hints,
                &mut hints.type_inlay_hints,
                &mut hints.parameter_inlay_hints,
                &mut hints.other_inlay_hints,
                &mut hints.padding_after_inlay_hints,
            ] {
                changes.update_positions(
                    annotations
                        .iter_mut()
                        .map(|annotation| (&mut annotation.char_idx, editor_core::Assoc::After)),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{document::test_document, ViewId};

    #[tokio::test]
    async fn view_removal_and_presentation_clones_keep_request_lifetimes_separate() {
        let mut doc = test_document("sample text");
        let mut views = slotmap::SlotMap::<ViewId, ()>::with_key();
        let first = views.insert(());
        let second = views.insert(());
        let key = CacheKey {
            version: doc.version(),
            uri: None,
            server: LanguageServerId::default(),
            viewport: DocumentInlayHintsId {
                first_line: 0,
                last_line: 1,
            },
        };
        let first_request = doc.view_state_mut(first).inlay_hints.start(key.clone());
        let second_request = doc.view_state_mut(second).inlay_hints.start(key.clone());
        doc.set_inlay_hints(second, DocumentInlayHints::empty_with_id(key.viewport));
        let preview = doc.presentation(&editor_core::Transaction::new(doc.text()), second);
        assert!(preview.inlay_hints(second).is_some());
        drop(preview);
        assert!(!second_request.is_canceled());

        doc.remove_view(first);
        assert!(first_request.is_canceled());
        assert!(!second_request.is_canceled());
        assert!(doc.inlay_hints(second).is_some());
        doc.reset_all_inlay_hints();
        assert!(second_request.is_canceled());
        assert!(doc.inlay_hints(second).is_none());
        assert!(doc.try_selection(second).is_some());
    }
}
