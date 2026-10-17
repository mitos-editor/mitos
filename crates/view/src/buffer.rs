//! Text state and undo groups, independent of document services and views.

use editor_core::{
    history::{History, HistoryStep, UndoKind},
    ChangeSet, Rope, Transaction,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("Changes require {actual} characters, but the buffer has {expected}")]
pub(crate) struct InvalidLength {
    pub expected: usize,
    pub actual: usize,
}

pub(crate) enum HistoryAction {
    Undo,
    Redo,
    Earlier(UndoKind),
    Later(UndoKind),
}

pub(crate) struct Buffer {
    text: Rope,
    version: i32,
    changes: ChangeSet,
    pending_original_text: Option<Rope>,
    history: History,
}

impl std::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer")
            .field("text", &self.text)
            .field("version", &self.version)
            .field("changes", &self.changes)
            .field("revision", &self.history.current_revision())
            .finish_non_exhaustive()
    }
}

impl Buffer {
    pub(crate) fn new(text: Rope) -> Self {
        Self {
            changes: ChangeSet::new(text.slice(..)),
            text,
            version: 0,
            pending_original_text: None,
            history: History::default(),
        }
    }

    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }
    pub(crate) fn version(&self) -> i32 {
        self.version
    }
    pub(crate) fn changes(&self) -> &ChangeSet {
        &self.changes
    }
    pub(crate) fn current_revision(&self) -> usize {
        self.history.current_revision()
    }
    pub(crate) fn current_transaction(&self) -> Option<&Transaction> {
        self.history.current_transaction()
    }
    pub(crate) fn changes_since(&self, revision: usize) -> Option<Transaction> {
        self.history.changes_since(revision)
    }

    pub(crate) fn validate_changes(&self, changes: &ChangeSet) -> Result<(), InvalidLength> {
        if changes.len_before() != self.text.len_chars() {
            return Err(InvalidLength {
                expected: self.text.len_chars(),
                actual: changes.len_before(),
            });
        }
        Ok(())
    }

    /// Mutate text; the document maps its own state before recording the undo group.
    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) -> Result<(), InvalidLength> {
        self.validate_changes(changes)?;
        if !changes.apply(&mut self.text) {
            return Err(InvalidLength {
                expected: self.text.len_chars(),
                actual: changes.len_before(),
            });
        }
        if !changes.is_empty() {
            self.version += 1;
        }
        Ok(())
    }

    pub(crate) fn record_changes(&mut self, before: Rope, changes: &ChangeSet) {
        if !changes.is_empty() {
            self.pending_original_text.get_or_insert(before);
            self.changes = std::mem::take(&mut self.changes).compose(changes.clone());
        }
    }

    pub(crate) fn commit_changes(&mut self) -> Option<usize> {
        if self.changes.is_empty() {
            return None;
        }
        let changes = std::mem::replace(&mut self.changes, ChangeSet::new(self.text.slice(..)));
        let original = self
            .pending_original_text
            .take()
            .expect("pending changes have original text");
        Some(self.history.commit_revision(&changes, &original))
    }

    pub(crate) fn history_steps(&mut self, action: HistoryAction) -> Vec<HistoryStep> {
        match action {
            HistoryAction::Undo => self.history.undo().into_iter().collect(),
            HistoryAction::Redo => self.history.redo().into_iter().collect(),
            HistoryAction::Earlier(kind) => self.history.earlier(kind),
            HistoryAction::Later(kind) => self.history.later(kind),
        }
    }

    pub(crate) fn clear_pending_changes(&mut self) {
        self.changes = ChangeSet::new(self.text.slice(..));
        self.pending_original_text = None;
    }
}
