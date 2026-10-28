//! Highlight cache and request lifetime for one document/view relationship.

use editor_core::{Assoc, ChangeSet};
use event::TaskController;

use crate::{Document, ViewId};

#[derive(Debug, Default)]
pub(crate) struct DocumentHighlightsState {
    cache: Option<DocumentHighlights>,
    request: TaskController,
}

impl DocumentHighlightsState {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet, text_len: usize) {
        if let Some(highlights) = &mut self.cache {
            highlights.ranges.retain_mut(|range| {
                changes.update_positions(
                    [
                        (&mut range.start, Assoc::After),
                        (&mut range.end, Assoc::After),
                    ]
                    .into_iter(),
                );
                range.end = range.end.min(text_len);
                range.start < range.end
            });
        }
    }
}

/// Highlight ranges returned by LSP `textDocument/documentHighlight` for a view.
#[derive(Debug, Clone, Default)]
pub struct DocumentHighlights {
    pub ranges: Vec<std::ops::Range<usize>>,
}

impl Document {
    pub fn set_document_highlights(
        &mut self,
        view_id: ViewId,
        ranges: Vec<std::ops::Range<usize>>,
    ) {
        if ranges.is_empty() {
            self.clear_document_highlights(view_id);
        } else {
            self.view_state_mut(view_id).highlights.cache = Some(DocumentHighlights { ranges });
        }
    }

    /// Clear rendered highlights without canceling a replacement request.
    pub fn clear_document_highlights(&mut self, view_id: ViewId) {
        if let Some(state) = self.view_states.get_mut(&view_id) {
            state.highlights.cache = None;
        }
    }

    /// Clear every view and cancel all outstanding highlight requests.
    pub fn clear_all_document_highlights(&mut self) {
        for state in self.view_states.values_mut() {
            state.highlights = Default::default();
        }
    }

    pub fn document_highlights(&self, view_id: ViewId) -> Option<&[std::ops::Range<usize>]> {
        Some(
            &self
                .view_states
                .get(&view_id)?
                .highlights
                .cache
                .as_ref()?
                .ranges,
        )
    }

    pub(crate) fn document_highlight_controller(&mut self, view_id: ViewId) -> &mut TaskController {
        &mut self.view_state_mut(view_id).highlights.request
    }
}
