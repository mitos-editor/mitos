//! Per-document/view cache ownership, freshness, cancellation, and edit mapping.

use std::{collections::HashMap, fmt};

use editor_core::{ChangeSet, Uri};
use event::{TaskController, TaskHandle};
use lsp_client::LanguageServerId;
use text_layout::text_annotations::InlineAnnotation;

use crate::ViewId;

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

#[derive(Debug, Default)]
struct ViewState {
    cache: Option<DocumentInlayHints>,
    request: RequestState,
}

/// One entry owns both cached annotations and their request lifetime.
#[derive(Debug, Default)]
pub(crate) struct InlayHintsState {
    views: HashMap<ViewId, ViewState>,
}

impl InlayHintsState {
    pub(crate) fn get(&self, view: ViewId) -> Option<&DocumentInlayHints> {
        self.views.get(&view)?.cache.as_ref()
    }

    pub(crate) fn set(&mut self, view: ViewId, hints: DocumentInlayHints) {
        self.views.entry(view).or_default().cache = Some(hints);
    }

    pub(crate) fn remove_view(&mut self, view: ViewId) {
        self.views.remove(&view);
    }

    pub(crate) fn clear(&mut self) {
        self.views.clear();
    }

    pub(super) fn is_current(&self, view: ViewId, key: &CacheKey) -> bool {
        self.views.get(&view).is_some_and(|state| {
            state.request.key.as_ref() == Some(key)
                && (state.request.completed || state.request.controller.is_running())
        })
    }

    pub(super) fn start(&mut self, view: ViewId, key: CacheKey) -> TaskHandle {
        let state = &mut self.views.entry(view).or_default().request;
        state.key = Some(key);
        state.completed = false;
        state.controller.restart()
    }

    pub(super) fn complete(&mut self, view: ViewId, hints: DocumentInlayHints) {
        let state = self
            .views
            .get_mut(&view)
            .expect("validated inlay request owns its view state");
        state.cache = Some(hints);
        state.request.completed = true;
    }

    pub(super) fn retry(&mut self, view: ViewId) {
        if let Some(state) = self.views.get_mut(&view) {
            state.request.invalidate();
        }
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        for state in self.views.values_mut() {
            state.request.invalidate();
            if let Some(hints) = &mut state.cache {
                for annotations in [
                    &mut hints.padding_before_inlay_hints,
                    &mut hints.type_inlay_hints,
                    &mut hints.parameter_inlay_hints,
                    &mut hints.other_inlay_hints,
                    &mut hints.padding_after_inlay_hints,
                ] {
                    changes.update_positions(
                        annotations.iter_mut().map(|annotation| {
                            (&mut annotation.char_idx, editor_core::Assoc::After)
                        }),
                    );
                }
            }
        }
    }
}
