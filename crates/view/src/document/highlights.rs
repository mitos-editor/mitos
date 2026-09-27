//! Per-view highlight caches and request cancellation.

use editor_core::{Assoc, ChangeSet};
use event::TaskController;
use std::collections::HashMap;

use super::Document;
use crate::{handlers::document_highlight::DocumentHighlightHandler, ViewId};

#[derive(Default)]
pub(crate) struct DocumentHighlightsState {
    cache: HashMap<ViewId, DocumentHighlights>,
    requests: HashMap<ViewId, TaskController>,
    pub(crate) handler: Option<DocumentHighlightHandler>,
}

impl DocumentHighlightsState {
    pub(super) fn remove_view(&mut self, view: ViewId) {
        self.cache.remove(&view);
        self.requests.remove(&view);
    }

    fn clear(&mut self) {
        self.cache.clear();
        self.requests.clear();
    }

    pub(super) fn apply_changes(&mut self, changes: &ChangeSet, text_len: usize) {
        for highlights in self.cache.values_mut() {
            let mut updated = Vec::with_capacity(highlights.ranges.len());
            for mut range in highlights.ranges.drain(..) {
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
            highlights.ranges = updated;
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
            self.document_highlights.cache.remove(&view_id);
        } else {
            self.document_highlights
                .cache
                .insert(view_id, DocumentHighlights { ranges });
        }
    }

    /// Clear rendered highlights without canceling a replacement request.
    pub fn clear_document_highlights(&mut self, view_id: ViewId) {
        self.document_highlights.cache.remove(&view_id);
    }

    /// Clear every view and cancel all outstanding highlight requests.
    pub fn clear_all_document_highlights(&mut self) {
        self.document_highlights.clear();
    }

    pub fn document_highlights(&self, view_id: ViewId) -> Option<&[std::ops::Range<usize>]> {
        self.document_highlights
            .cache
            .get(&view_id)
            .map(|highlights| highlights.ranges.as_slice())
    }

    pub(crate) fn document_highlight_controller(&mut self, view_id: ViewId) -> &mut TaskController {
        self.document_highlights
            .requests
            .entry(view_id)
            .or_default()
    }
}
