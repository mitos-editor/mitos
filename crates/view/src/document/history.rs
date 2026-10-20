//! Text undo groups with optional, separately stored editor selection snapshots.

use super::Document;
use crate::ViewId;
use buffer::history::{HistoryStep, UndoKind};

use buffer::HistoryAction;

impl Document {
    /// Commit pending text as one undo revision. No view or cursor is required.
    pub fn commit_changes(&mut self) -> Option<usize> {
        let revision = self.buffer.commit_changes();
        self.selection_history.commit(revision, |view| {
            self.view_states
                .get(&view)
                .map(|state| state.selection.clone())
        });
        revision
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
        }
        let steps = self.buffer.prepare_history(action);
        self.replay_history(steps, origin)
    }

    fn replay_history(&mut self, steps: Vec<HistoryStep>, origin: Option<ViewId>) -> bool {
        let mut applied = false;
        for step in steps {
            let selection = origin
                .and_then(|_| self.selection_history.selection(&step))
                .cloned();
            let Ok(change) = self.buffer.apply_history_step(&step) else {
                // An observer may have edited or committed after the previous step.
                break;
            };
            // History replay ends interactive completion and snippet sessions.
            self.accepted_completions.cancel();
            self.clear_snippet();
            self.publish_change(&change, selection.as_ref(), origin, true);
            applied = true;
        }
        applied
    }
}
