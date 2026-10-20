//! Live inverse changes for temporary text restoration, independent of views.

use std::sync::{Arc, Mutex, Weak};

use editor_core::{ChangeSet, Rope};

use crate::{AppliedChange, Buffer, InvalidLength};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SavePointError {
    #[error("Savepoint belongs to another buffer")]
    WrongBuffer,
    #[error(transparent)]
    InvalidLength(#[from] InvalidLength),
}

/// A text checkpoint kept current by its originating buffer. It does not keep
/// the buffer alive and cannot be used with another buffer, even with equal text.
#[derive(Debug)]
pub struct SavePoint {
    owner: Weak<()>,
    inverse: Mutex<ChangeSet>,
}

impl SavePoint {
    /// Changes restoring the checkpoint, without retaining a lock during mutation
    /// or observer callbacks. The buffer also tracks edits made by restoration.
    pub fn revert(&self, buffer: &Buffer) -> Result<ChangeSet, SavePointError> {
        if !Weak::ptr_eq(&self.owner, &Arc::downgrade(&buffer.identity)) {
            return Err(SavePointError::WrongBuffer);
        }
        Ok(self
            .inverse
            .lock()
            .expect("savepoint lock poisoned")
            .clone())
    }

    pub fn saved_text(&self, buffer: &Buffer) -> Result<Rope, SavePointError> {
        let revert = self.revert(buffer)?;
        buffer.validate_changes(&revert)?;
        let mut text = buffer.text().clone();
        revert.apply(&mut text);
        Ok(text)
    }

    /// Forward changes from the saved text to the buffer's current text.
    pub fn changes_since(&self, buffer: &Buffer) -> Result<ChangeSet, SavePointError> {
        Ok(self.revert(buffer)?.invert(buffer.text()))
    }
}

impl Buffer {
    pub fn savepoint(&mut self) -> Arc<SavePoint> {
        self.savepoints.retain(|saved| saved.strong_count() != 0);
        // Reuse the latest unchanged checkpoint. This keeps completion providers
        // sharing one inverse when they start from the same text.
        if let Some(saved) = self.savepoints.iter().rev().find_map(Weak::upgrade)
            && saved
                .inverse
                .lock()
                .expect("savepoint lock poisoned")
                .is_empty()
        {
            return saved;
        }
        let saved = Arc::new(SavePoint {
            owner: Arc::downgrade(&self.identity),
            inverse: Mutex::new(ChangeSet::new(self.text().slice(..))),
        });
        self.savepoints.push(Arc::downgrade(&saved));
        saved
    }

    /// Restore text as an ordinary pending edit, so undo can recover the changes.
    pub fn restore(&mut self, savepoint: &SavePoint) -> Result<AppliedChange, SavePointError> {
        let revert = savepoint.revert(self)?;
        Ok(self.apply(&revert)?)
    }

    pub(super) fn map_savepoints(&mut self, changes: &ChangeSet, before: &Rope) {
        if self.savepoints.is_empty() {
            return;
        }
        let revert = changes.invert(before);
        self.savepoints.retain(|saved| {
            let Some(saved) = saved.upgrade() else {
                return false;
            };
            let mut inverse = saved.inverse.lock().expect("savepoint lock poisoned");
            *inverse = revert.clone().compose(std::mem::take(&mut inverse));
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HistoryAction;
    use editor_core::Transaction;

    fn replace(buffer: &mut Buffer, from: usize, to: usize, text: &str) {
        let edit = Transaction::change(buffer.text(), [(from, to, Some(text.into()))].into_iter());
        buffer.apply(edit.changes()).unwrap();
    }

    #[test]
    fn savepoints_restore_unicode_text_repeatedly_and_preserve_other_checkpoints() {
        let mut buffer = Buffer::new(Rope::from("a界\r\n"));
        let original = buffer.savepoint();
        replace(&mut buffer, 0, 1, "é🦀");
        let second = buffer.savepoint();
        replace(&mut buffer, 2, 3, "text");
        assert_eq!(original.saved_text(&buffer).unwrap(), "a界\r\n");
        assert_eq!(second.saved_text(&buffer).unwrap(), "é🦀界\r\n");
        let mut saved = original.saved_text(&buffer).unwrap();
        original.changes_since(&buffer).unwrap().apply(&mut saved);
        assert_eq!(&saved, buffer.text());
        buffer.restore(&original).unwrap();
        assert_eq!(buffer.text(), "a界\r\n");
        buffer.restore(&original).unwrap();
        buffer.restore(&second).unwrap();
        assert_eq!(buffer.text(), "é🦀界\r\n");
        assert_eq!(original.saved_text(&buffer).unwrap(), "a界\r\n");
    }

    #[test]
    fn history_replay_updates_checkpoints_and_restoration_is_undoable() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        replace(&mut buffer, 0, 1, "界");
        buffer.commit_changes();
        let saved = buffer.savepoint();
        replace(&mut buffer, 1, 2, "é");
        buffer.commit_changes();
        buffer.restore(&saved).unwrap();
        buffer.commit_changes();
        for action in [
            HistoryAction::Undo,
            HistoryAction::Undo,
            HistoryAction::Undo,
            HistoryAction::Redo,
        ] {
            for step in buffer.prepare_history(action) {
                buffer.apply_history_step(&step).unwrap();
                assert_eq!(saved.saved_text(&buffer).unwrap(), "界bc");
            }
        }
        buffer.restore(&saved).unwrap();
        assert_eq!(buffer.text(), "界bc");
    }

    #[test]
    fn foreign_checkpoints_and_rejected_changes_preserve_text_and_history() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        let saved = buffer.savepoint();
        let mut other = Buffer::new(Rope::from("abc"));
        assert_eq!(saved.saved_text(&other), Err(SavePointError::WrongBuffer));
        assert_eq!(
            other.restore(&saved).unwrap_err(),
            SavePointError::WrongBuffer
        );
        assert_eq!(other.text(), "abc");
        assert_eq!(other.version(), 0);
        assert!(other.changes().is_empty());
        replace(&mut buffer, 0, 1, "界");
        let pending = buffer.changes().clone();
        assert!(buffer
            .apply(&ChangeSet::new(Rope::from("wrong").slice(..)))
            .is_err());
        assert_eq!(buffer.changes(), &pending);
        assert_eq!(saved.saved_text(&buffer).unwrap(), "abc");
    }

    #[test]
    fn unchanged_checkpoints_share_tracking_and_expired_checkpoints_are_released() {
        let mut buffer = Buffer::new(Rope::from("abc"));
        let first = buffer.savepoint();
        let second = buffer.savepoint();
        assert!(Arc::ptr_eq(&first, &second));
        let expired = Arc::downgrade(&first);
        drop(first);
        drop(second);
        let _next = buffer.savepoint();
        assert!(expired.upgrade().is_none());
        assert_eq!(buffer.savepoints.len(), 1);
    }
}
