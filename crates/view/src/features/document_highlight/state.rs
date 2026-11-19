//! Per-view highlight cache and request cancellation.

use editor_core::{Assoc, ChangeSet};
use event::TaskController;

use crate::{Document, ViewId};

#[derive(Debug, Default)]
pub(crate) struct DocumentHighlightsState {
    cache: Option<Vec<std::ops::Range<usize>>>,
    request: Option<TaskController>,
}

impl DocumentHighlightsState {
    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet, text_len: usize) {
        if let Some(highlights) = &mut self.cache {
            Self::map(highlights, changes, text_len);
        }
    }
    pub(crate) fn map(
        highlights: &mut Vec<std::ops::Range<usize>>,
        changes: &ChangeSet,
        text_len: usize,
    ) {
        let mut updated = Vec::with_capacity(highlights.len());
        for mut range in highlights.drain(..) {
            changes.update_positions(
                [
                    (&mut range.start, Assoc::After),
                    (&mut range.end, Assoc::After),
                ]
                .into_iter(),
            );
            if range.start >= text_len {
                continue;
            }
            let end = range.end.min(text_len);
            if range.start < end {
                updated.push(range.start..end);
            }
        }
        *highlights = updated;
    }
}

impl Document {
    pub fn set_document_highlights(
        &mut self,
        view_id: ViewId,
        ranges: Vec<std::ops::Range<usize>>,
    ) {
        self.view_state_mut(view_id).document_highlights.cache =
            (!ranges.is_empty()).then_some(ranges);
    }

    /// Clear rendered highlights without canceling a replacement request.
    pub fn clear_document_highlights(&mut self, view_id: ViewId) {
        if let Some(state) = self.view_states.get_mut(&view_id) {
            state.document_highlights.cache = None;
        }
    }

    /// Clear every view and cancel all outstanding highlight requests.
    pub fn clear_all_document_highlights(&mut self) {
        for state in self.view_states.values_mut() {
            state.document_highlights = Default::default();
        }
    }

    pub fn document_highlights(&self, view_id: ViewId) -> Option<&[std::ops::Range<usize>]> {
        self.view_states
            .get(&view_id)?
            .document_highlights
            .cache
            .as_deref()
    }

    pub(crate) fn document_highlight_controller(&mut self, view_id: ViewId) -> &mut TaskController {
        self.view_state_mut(view_id)
            .document_highlights
            .request
            .get_or_insert_default()
    }
}
