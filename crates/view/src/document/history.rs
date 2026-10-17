//! Text undo groups with optional, separately stored editor selection snapshots.

use super::Document;
use crate::ViewId;
use editor_core::{
    history::{HistoryStep, UndoKind},
    ChangeSet,
};

pub(crate) enum HistoryAction {
    Undo,
    Redo,
    Earlier(UndoKind),
    Later(UndoKind),
}

impl Document {
    /// Commit pending text as one undo revision. No view or cursor is required.
    pub fn commit_changes(&mut self) -> Option<usize> {
        if self.changes.is_empty() {
            return None;
        }
        let changes = std::mem::replace(&mut self.changes, ChangeSet::new(self.text.slice(..)));
        let original = self
            .pending_original_text
            .take()
            .expect("pending changes have original text");
        let revision = self.history.get_mut().commit_revision(&changes, &original);
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
        } else if !self.changes.is_empty() {
            return false;
        }
        let steps = match action {
            HistoryAction::Undo => self.history.get_mut().undo().into_iter().collect(),
            HistoryAction::Redo => self.history.get_mut().redo().into_iter().collect(),
            HistoryAction::Earlier(kind) => self.history.get_mut().earlier(kind),
            HistoryAction::Later(kind) => self.history.get_mut().later(kind),
        };
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
            self.changes = ChangeSet::new(self.text.slice(..));
        }
        applied
    }
}
