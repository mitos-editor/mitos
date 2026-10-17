//! Text state and undo groups, independent of document services and views.

use std::sync::Arc;

use editor_core::{
    history::{History, HistoryStep},
    ChangeSet, Rope, Transaction,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("Changes require {actual} characters, but the buffer has {expected}")]
pub struct InvalidLength {
    pub expected: usize,
    pub actual: usize,
}

pub use editor_core::history::HistoryAction;

/// Rejections leave text, versions, pending groups, and the history cursor unchanged.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error(transparent)]
    InvalidLength(#[from] InvalidLength),
    #[error("Cannot replay history while edits are pending")]
    PendingChanges,
    #[error("History step is no longer applicable")]
    InvalidStep,
}

/// Text change already applied to a buffer. The caller synchronously updates its
/// derived state before publishing the change to observers.
#[derive(Debug)]
pub struct AppliedChange {
    before: Rope,
    changes: ChangeSet,
}

impl AppliedChange {
    pub fn before(&self) -> &Rope {
        &self.before
    }
    pub fn changes(&self) -> &ChangeSet {
        &self.changes
    }
}

/// Immutable text, version, and committed revision captured from one buffer.
/// Clones share rope storage. Committing an undo group does not invalidate its text;
/// any non-empty edit or history replay does, even when it restores identical text.
#[derive(Clone, Debug)]
pub struct Snapshot {
    text: Rope,
    version: i32,
    revision: usize,
    identity: Arc<()>,
}

impl Snapshot {
    pub fn text(&self) -> &Rope {
        &self.text
    }
    pub fn version(&self) -> i32 {
        self.version
    }
    /// Committed revision at capture; the snapshot may also contain pending edits.
    pub fn revision(&self) -> usize {
        self.revision
    }
    /// Text identity only. Applications separately validate paths, configuration,
    /// protocol sessions, cancellation, and view bindings when publishing results.
    pub fn is_current(&self, buffer: &Buffer) -> bool {
        Arc::ptr_eq(&self.identity, &buffer.identity) && self.version == buffer.version
    }
}

pub struct Buffer {
    identity: Arc<()>,
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
    pub fn new(text: Rope) -> Self {
        Self {
            identity: Arc::new(()),
            changes: ChangeSet::new(text.slice(..)),
            text,
            version: 0,
            pending_original_text: None,
            history: History::default(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            version: self.version,
            revision: self.current_revision(),
            identity: self.identity.clone(),
        }
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }
    pub fn version(&self) -> i32 {
        self.version
    }
    pub fn changes(&self) -> &ChangeSet {
        &self.changes
    }
    pub fn current_revision(&self) -> usize {
        self.history.current_revision()
    }
    pub fn current_transaction(&self) -> Option<&Transaction> {
        self.history.current_transaction()
    }
    pub fn changes_since(&self, revision: usize) -> Option<Transaction> {
        self.history.changes_since(revision)
    }

    pub fn validate_changes(&self, changes: &ChangeSet) -> Result<(), InvalidLength> {
        if changes.len_before() != self.text.len_chars() {
            return Err(InvalidLength {
                expected: self.text.len_chars(),
                actual: changes.len_before(),
            });
        }
        Ok(())
    }

    /// Apply text and record its pending undo group before returning to the caller.
    pub fn apply(&mut self, changes: &ChangeSet) -> Result<AppliedChange, InvalidLength> {
        let applied = self.apply_changes(changes)?;
        if !changes.is_empty() {
            self.pending_original_text
                .get_or_insert_with(|| applied.before.clone());
            self.changes = std::mem::take(&mut self.changes).compose(changes.clone());
        }
        Ok(applied)
    }

    fn apply_changes(&mut self, changes: &ChangeSet) -> Result<AppliedChange, InvalidLength> {
        self.validate_changes(changes)?;
        let before = self.text.clone();
        if !changes.apply(&mut self.text) {
            return Err(InvalidLength {
                expected: self.text.len_chars(),
                actual: changes.len_before(),
            });
        }
        if !changes.is_empty() {
            self.version += 1;
        }
        Ok(AppliedChange {
            before,
            changes: changes.clone(),
        })
    }

    /// Apply a planned history step and advance its revision together. New pending
    /// edits or an out-of-order step reject the replay rather than discarding edits.
    pub fn apply_history_step(&mut self, step: &HistoryStep) -> Result<AppliedChange, ReplayError> {
        if !self.changes.is_empty() {
            return Err(ReplayError::PendingChanges);
        }
        self.validate_changes(step.transaction.changes())?;
        if !self.history.advance(step) {
            return Err(ReplayError::InvalidStep);
        }
        // ChangeSet::apply only rejects a mismatched length, already validated above.
        let applied = self.apply_changes(step.transaction.changes())?;
        self.clear_pending_changes();
        Ok(applied)
    }

    pub fn commit_changes(&mut self) -> Option<usize> {
        if self.changes.is_empty() {
            self.pending_original_text = None;
            return None;
        }
        let changes = std::mem::replace(&mut self.changes, ChangeSet::new(self.text.slice(..)));
        let original = self
            .pending_original_text
            .take()
            .expect("pending changes have original text");
        Some(self.history.commit_revision(&changes, &original))
    }

    /// Plan history without advancing its current revision. Apply the steps in order
    /// with `apply_history_step`, updating derived state between steps.
    /// Undo/earlier commit pending text; redo/later refuse to discard pending edits.
    pub fn prepare_history(&mut self, action: HistoryAction) -> Vec<HistoryStep> {
        if matches!(action, HistoryAction::Undo | HistoryAction::Earlier(_)) {
            self.commit_changes();
        } else if !self.changes.is_empty() {
            return Vec::new();
        }
        self.history.plan(action)
    }

    fn clear_pending_changes(&mut self) {
        self.changes = ChangeSet::new(self.text.slice(..));
        self.pending_original_text = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::history::UndoKind;

    fn replace(buffer: &mut Buffer, from: usize, to: usize, text: &str) -> AppliedChange {
        let transaction =
            Transaction::change(buffer.text(), [(from, to, Some(text.into()))].into_iter());
        buffer.apply(transaction.changes()).unwrap()
    }

    fn navigate(buffer: &mut Buffer, action: HistoryAction) {
        for step in buffer.prepare_history(action) {
            buffer.apply_history_step(&step).unwrap();
        }
    }

    #[test]
    fn successive_edits_share_an_undo_group_and_replay_without_pending_changes() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        let change = replace(&mut buffer, 0, 1, "α");
        assert_eq!(change.before(), "abc");
        let mut original = change.before().clone();
        assert!(change.changes().apply(&mut original));
        assert_eq!(&original, buffer.text());
        replace(&mut buffer, 1, 2, "界");
        assert_eq!(buffer.version(), 2);
        let revision = buffer.commit_changes().unwrap();
        assert_eq!(buffer.current_revision(), revision);
        navigate(&mut buffer, HistoryAction::Undo);
        assert_eq!(buffer.text(), "abc");
        assert!(buffer.changes().is_empty());
        navigate(&mut buffer, HistoryAction::Redo);
        assert_eq!(buffer.text(), "α界c");
        assert_eq!(buffer.version(), 4);
        assert_eq!(buffer.commit_changes(), None);
    }

    #[test]
    fn rejected_changes_and_empty_edits_preserve_text_version_and_pending_group() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        replace(&mut buffer, 0, 0, "x");
        let pending = buffer.changes().clone();
        let invalid = ChangeSet::new(Rope::from("wrong length").slice(..));
        assert_eq!(
            buffer.apply(&invalid).unwrap_err(),
            InvalidLength {
                expected: 4,
                actual: 12
            }
        );
        let identity = ChangeSet::new(buffer.text().slice(..));
        buffer.apply(&identity).unwrap();
        assert_eq!(buffer.text(), "xabc");
        assert_eq!(buffer.version(), 1);
        assert_eq!(buffer.changes(), &pending);
        navigate(&mut buffer, HistoryAction::Undo);
        assert_eq!(buffer.text(), "abc");
    }

    #[test]
    fn chronological_history_crosses_branches_without_views() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        replace(&mut buffer, 0, 1, "A");
        buffer.commit_changes();
        replace(&mut buffer, 1, 2, "B");
        buffer.commit_changes();
        navigate(&mut buffer, HistoryAction::Undo);
        replace(&mut buffer, 1, 2, "Z");
        buffer.commit_changes();
        navigate(&mut buffer, HistoryAction::Earlier(UndoKind::Steps(1)));
        assert_eq!(buffer.text(), "ABc");
        navigate(&mut buffer, HistoryAction::Later(UndoKind::Steps(1)));
        assert_eq!(buffer.text(), "AZc");
        assert!(buffer.changes().is_empty());
    }
    #[test]
    fn snapshots_keep_captured_text_and_survive_history_commits() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        replace(&mut buffer, 0, 1, "A");
        let snapshot = buffer.snapshot();
        assert_eq!(snapshot.text(), "Abc");
        assert_eq!(snapshot.version(), 1);
        assert_eq!(snapshot.revision(), 0);
        buffer.commit_changes();
        assert!(snapshot.is_current(&buffer));
        replace(&mut buffer, 1, 2, "B");
        assert_eq!(snapshot.text(), "Abc");
        assert!(!snapshot.is_current(&buffer));
    }

    #[test]
    fn equal_text_and_versions_cannot_revive_or_cross_buffer_snapshots() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        replace(&mut buffer, 0, 1, "A");
        let snapshot = buffer.snapshot();
        let mut other = Buffer::new(Rope::from("abc"));
        replace(&mut other, 0, 1, "A");
        assert_eq!(snapshot.version(), other.version());
        assert_eq!(snapshot.text(), other.text());
        assert!(!snapshot.is_current(&other));
        navigate(&mut buffer, HistoryAction::Undo);
        navigate(&mut buffer, HistoryAction::Redo);
        assert_eq!(snapshot.text(), buffer.text());
        assert!(!snapshot.is_current(&buffer));
    }
    #[test]
    fn planned_replay_keeps_each_text_snapshot_at_its_own_revision() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        replace(&mut buffer, 0, 1, "A");
        let first = buffer.commit_changes().unwrap();
        replace(&mut buffer, 1, 2, "B");
        let second = buffer.commit_changes().unwrap();
        navigate(&mut buffer, HistoryAction::Undo);
        replace(&mut buffer, 1, 2, "Z");
        let branch = buffer.commit_changes().unwrap();
        let steps = buffer.prepare_history(HistoryAction::Earlier(UndoKind::Steps(1)));
        assert_eq!(buffer.snapshot().revision(), branch);
        assert_eq!(buffer.text(), "AZc");
        assert_eq!(steps.len(), 2);
        buffer.apply_history_step(&steps[0]).unwrap();
        assert_eq!(buffer.snapshot().revision(), first);
        assert_eq!(buffer.text(), "Abc");
        buffer.apply_history_step(&steps[1]).unwrap();
        assert_eq!(buffer.snapshot().revision(), second);
        assert_eq!(buffer.text(), "ABc");
    }

    #[test]
    fn interrupted_replay_preserves_pending_edits_and_rejects_stale_steps() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        replace(&mut buffer, 0, 1, "A");
        let revision = buffer.commit_changes().unwrap();
        let steps = buffer.prepare_history(HistoryAction::Undo);
        replace(&mut buffer, 0, 1, "Q");
        assert_eq!(
            buffer.apply_history_step(&steps[0]).unwrap_err(),
            ReplayError::PendingChanges
        );
        assert_eq!(buffer.text(), "Qbc");
        assert_eq!(buffer.current_revision(), revision);
        buffer.commit_changes();
        let current = buffer.snapshot();
        assert_eq!(
            buffer.apply_history_step(&steps[0]).unwrap_err(),
            ReplayError::InvalidStep
        );
        assert!(current.is_current(&buffer));
        assert_eq!(buffer.current_revision(), current.revision());
    }
}
