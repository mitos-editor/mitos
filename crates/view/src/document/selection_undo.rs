//! Per-view history of selection changes, so a mistyped command that replaces a
//! carefully built set of selections can be undone without touching the text.
//!
//! Every selection change made without editing records the previous selection.
//! Editing the text clears the history of every view, because stored ranges no
//! longer describe the same text; undoing the edit already restores its selection.

use editor_core::Selection;

use super::Document;
use crate::ViewId;

/// Older entries are dropped first. Multi-cursor selections can be large.
const LIMIT: usize = 100;

#[derive(Debug, Default)]
pub(crate) struct SelectionUndo {
    /// Selections before the current one, newest last.
    past: Vec<Selection>,
    /// Original selection while a prompt previews changes.
    preview: Option<Selection>,
    /// Selections undone from the current one, the next one to restore last.
    future: Vec<Selection>,
}

impl SelectionUndo {
    /// The view moved from `previous` to a new selection.
    pub(super) fn record(&mut self, previous: Selection) {
        if self.preview.is_some() {
            return;
        }
        if self.past.len() == LIMIT {
            self.past.remove(0);
        }
        self.past.push(previous);
        self.future.clear();
    }

    pub(super) fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
        self.preview = None;
    }

    /// Step `count` entries back (`undo`) or forward, as far as the history goes,
    /// and return the selection to show. `None` when there is nothing to step to.
    fn step(&mut self, current: &Selection, count: usize, undo: bool) -> Option<Selection> {
        let (from, to) = if undo {
            (&mut self.past, &mut self.future)
        } else {
            (&mut self.future, &mut self.past)
        };
        let count = count.min(from.len());
        if count == 0 {
            return None;
        }
        to.push(current.clone());
        let mut target = from.pop()?;
        for _ in 1..count {
            to.push(target);
            target = from.pop()?;
        }
        Some(target)
    }
}

impl Document {
    /// Defer history recording while a prompt previews selection changes.
    pub fn begin_selection_preview(&mut self, view: ViewId) {
        if let Some(state) = self.view_states.get_mut(&view) {
            state.selection_undo.preview = Some(state.selection.clone());
        }
    }

    /// Record the final selection as one change. Restore the original selection
    /// before calling this when canceling a preview.
    pub fn finish_selection_preview(&mut self, view: ViewId) {
        if let Some(state) = self.view_states.get_mut(&view)
            && let Some(original) = state.selection_undo.preview.take()
            && original != state.selection
        {
            state.selection_undo.record(original);
        }
    }

    /// Restore the selection from `count` changes ago. Returns `false` when there
    /// is no older selection for this view.
    pub fn undo_selection(&mut self, view: ViewId, count: usize) -> bool {
        self.step_selection(view, count, true)
    }

    /// Reapply `count` undone selection changes. Returns `false` when there is no
    /// newer selection for this view.
    pub fn redo_selection(&mut self, view: ViewId, count: usize) -> bool {
        self.step_selection(view, count, false)
    }

    fn step_selection(&mut self, view: ViewId, count: usize, undo: bool) -> bool {
        let Some(state) = self.view_states.get_mut(&view) else {
            return false;
        };
        let mut history = std::mem::take(&mut state.selection_undo);
        let Some(target) = history.step(&state.selection, count, undo) else {
            state.selection_undo = history;
            return false;
        };
        // Selecting would record the current selection; install the stepped history after.
        self.set_selection(view, target);
        if let Some(state) = self.view_states.get_mut(&view) {
            state.selection_undo = history;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::Range;

    fn point(pos: usize) -> Selection {
        Selection::point(pos)
    }

    #[test]
    fn steps_back_and_forward_by_count_and_stops_at_either_end() {
        let mut history = SelectionUndo::default();
        for pos in 0..3 {
            history.record(point(pos));
        }
        // Current is 3; history holds 0, 1, 2.
        assert_eq!(history.step(&point(3), 2, true), Some(point(1)));
        assert_eq!(history.step(&point(1), 5, true), Some(point(0)));
        assert_eq!(history.step(&point(0), 1, true), None);
        assert_eq!(history.step(&point(0), 3, false), Some(point(3)));
        assert_eq!(history.step(&point(3), 1, false), None);
    }

    #[test]
    fn a_new_selection_after_undo_drops_the_redo_entries() {
        let mut history = SelectionUndo::default();
        history.record(point(0));
        assert_eq!(history.step(&point(1), 1, true), Some(point(0)));
        history.record(point(0));
        assert_eq!(history.step(&point(5), 1, false), None);
        assert_eq!(history.step(&point(5), 1, true), Some(point(0)));
    }

    #[test]
    fn history_keeps_only_the_newest_entries() {
        let mut history = SelectionUndo::default();
        for pos in 0..LIMIT + 5 {
            history.record(Selection::single(pos, pos + 1));
        }
        assert_eq!(history.past.len(), LIMIT);
        assert_eq!(history.past[0].primary(), Range::new(5, 6));
    }
}
