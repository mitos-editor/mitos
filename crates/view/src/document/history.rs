//! Text undo groups with optional, separately stored editor selection snapshots.

use super::Document;
use crate::ViewId;
use editor_core::history::{HistoryStep, UndoKind};

use crate::buffer::HistoryAction;

impl Document {
    /// Commit pending text as one undo revision. No view or cursor is required.
    pub fn commit_changes(&mut self) -> Option<usize> {
        let revision = self.buffer.commit_changes()?;
        self.selection_history.commit(revision, &self.selections);
        Some(revision)
    }

    /// Undo text without restoring a particular view's cursor.
    pub fn undo(&mut self) -> bool {
        self.change_history(HistoryAction::Undo, None)
    }

    pub fn redo(&mut self) -> bool {
        self.change_history(HistoryAction::Redo, None)
    }

    pub fn earlier(&mut self, kind: UndoKind) -> bool {
        self.change_history(HistoryAction::Earlier(kind), None)
    }

    pub fn later(&mut self, kind: UndoKind) -> bool {
        self.change_history(HistoryAction::Later(kind), None)
    }

    pub(crate) fn change_history(&mut self, action: HistoryAction, origin: Option<ViewId>) -> bool {
        if self.is_binary() {
            return false;
        }
        if matches!(action, HistoryAction::Undo | HistoryAction::Earlier(_)) {
            self.commit_changes();
        } else if !self.buffer.changes().is_empty() {
            return false;
        }
        let steps = self.buffer.history_steps(action);
        self.replay_history(steps, origin)
    }

    fn replay_history(&mut self, steps: Vec<HistoryStep>, origin: Option<ViewId>) -> bool {
        let mut applied = false;
        for step in steps {
            let transaction = match origin.and_then(|_| self.selection_history.selection(&step)) {
                Some(selection) => step.transaction.with_selection(selection.clone()),
                None => step.transaction,
            };
            applied |= self.apply_impl(&transaction, origin, true).is_ok();
        }
        if applied {
            self.buffer.clear_pending_changes();
        }
        applied
    }
}
