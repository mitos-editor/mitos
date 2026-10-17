//! Mandatory document invariants established before notifying observers.

use super::Document;
use crate::ViewId;
use editor_core::{Assoc, ChangeSet, Selection};

impl Document {
    pub(super) fn update_selection(&mut self, view: ViewId, selection: Selection) {
        self.selections.insert(
            view,
            selection.ensure_invariants(self.buffer.text().slice(..)),
        );
        if self
            .active_snippet
            .as_ref()
            .is_some_and(|snippet| !snippet.is_valid(self.selection(view)))
        {
            self.active_snippet = None;
        }
    }

    pub(super) fn map_snippet(&mut self, changes: &ChangeSet) {
        if self
            .active_snippet
            .as_mut()
            .is_some_and(|snippet| snippet.map(changes))
        {
            self.active_snippet = None;
        }
    }

    pub(super) fn map_diagnostics(&mut self, changes: &ChangeSet) {
        // map diagnostics over changes too
        changes.update_positions(self.diagnostics.iter_mut().map(|diagnostic| {
            let assoc = if diagnostic.starts_at_word {
                Assoc::BeforeWord
            } else {
                Assoc::After
            };
            (&mut diagnostic.range.start, assoc)
        }));
        changes.update_positions(self.diagnostics.iter_mut().filter_map(|diagnostic| {
            if diagnostic.zero_width {
                // for zero width diagnostics treat the diagnostic as a point
                // rather than a range
                return None;
            }
            let assoc = if diagnostic.ends_at_word {
                Assoc::AfterWord
            } else {
                Assoc::Before
            };
            Some((&mut diagnostic.range.end, assoc))
        }));
        self.diagnostics.retain_mut(|diagnostic| {
            if diagnostic.zero_width {
                diagnostic.range.end = diagnostic.range.start
            } else if diagnostic.range.start >= diagnostic.range.end {
                return false;
            }
            diagnostic.line = self.buffer.text().char_to_line(diagnostic.range.start);
            true
        });

        self.diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range,
                diagnostic.severity,
                diagnostic.provider.clone(),
            )
        });
    }
}
