//! Quicklist state and traversal helpers.
//!
//! The quicklist is an editor-global list of locations captured from pickers or views.
//! It is intended for workflows where a picker already shows a useful set of
//! files or file locations and the user wants to keep traversing that set after
//! the picker is closed.
//!
//! Typical usage looks like this:
//! - A picker is opened and filtered down to a useful result set.
//! - The picker copies its current matched items into the quicklist.
//! - The user navigates those stored entries forward or backward, globally.
//!
//! Quicklist entries can target either paths on disk or open in-memory
//! documents. Each entry also carries a position payload describing how that
//! target should be restored:
//! - `QuicklistPosition::Selection` preserves an exact selection.
//! - `QuicklistPosition::LineRange` stores a coarse zero-based line span.
//! - `QuicklistPosition::LineColRange` stores a zero-based line/column range.
//! - `QuicklistPosition::LspRange` stores an LSP-native range with its offset
//!   encoding for exact restoration.
//! - `QuicklistPosition::None` reopens the target without overriding the
//!   document's existing restored cursor/selection.
//!

use crate::DocumentId;
use editor_core::{movement::Direction, RopeSlice, Selection};
use lsp_client::{lsp, OffsetEncoding};
use std::path::{Path, PathBuf};

/// The destination referenced by a quicklist entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuicklistTarget {
    /// A file on disk.
    Path(PathBuf),
    /// An open in-memory document, including unsaved buffers.
    Document(DocumentId),
}

/// The position to restore when activating a quicklist entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuicklistPosition {
    /// Exact selection.
    Selection(Selection),
    /// Coarse zero-based line range.
    LineRange { start: usize, end: usize },
    /// Zero-based line/column range.
    LineColRange {
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    },
    /// Exact LSP range using the original offset encoding.
    //
    // NOTE: shouldn't be needed if we are able to perform
    // offset encoding right in the LSP client and work with
    // editor-specific offsets outside of it. That's unfortunately
    // not the case at the moment.
    LspRange {
        range: lsp::Range,
        offset_encoding: OffsetEncoding,
    },
    /// Do not override the document's existing restored selection.
    None,
}

impl QuicklistPosition {
    /// Returns a best-effort zero-based line range for preview and display.
    pub fn line_range(&self, text: Option<RopeSlice<'_>>) -> Option<(usize, usize)> {
        match self {
            Self::Selection(selection) => text.map(|text| selection.primary().line_range(text)),
            Self::LineRange { start, end } => Some((*start, *end)),
            Self::LineColRange {
                start_line,
                end_line,
                ..
            } => Some((*start_line, *end_line)),
            Self::LspRange { range, .. } => {
                Some((range.start.line as usize, range.end.line as usize))
            }
            Self::None => None,
        }
    }
}

/// A stored location in the quicklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicklistEntry {
    /// The file or document to visit.
    pub target: QuicklistTarget,
    /// The position to restore within the target.
    pub position: QuicklistPosition,
}

/// An editor-global list of explicitly captured locations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Quicklist {
    entries: Vec<QuicklistEntry>,
    cursor: Option<QuicklistCursor>,
    revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuicklistCursor {
    Entry(usize),
    /// The gap left by a removed current entry, before the entry at this index.
    Gap(usize),
}

pub(crate) struct QuicklistMatch<'a> {
    pub index: usize,
    pub entry: &'a QuicklistEntry,
}

impl Quicklist {
    /// Replaces the quicklist contents and clears the current position.
    pub fn replace(&mut self, entries: Vec<QuicklistEntry>) {
        self.entries = entries;
        self.cursor = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Appends a location without changing the current navigation position.
    pub fn push(&mut self, entry: QuicklistEntry) {
        self.entries.push(entry);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Removes an entry without moving the editor's selection. Navigation from a
    /// removed current entry continues on either side of the resulting gap.
    pub fn remove(&mut self, index: usize) -> Option<QuicklistEntry> {
        if index >= self.entries.len() {
            return None;
        }
        let entry = self.entries.remove(index);
        self.revision = self.revision.wrapping_add(1);
        self.cursor = if self.entries.is_empty() {
            None
        } else {
            self.cursor.map(|cursor| match cursor {
                QuicklistCursor::Entry(current) if current == index => QuicklistCursor::Gap(index),
                QuicklistCursor::Entry(current) => {
                    QuicklistCursor::Entry(current - usize::from(index < current))
                }
                QuicklistCursor::Gap(gap) => QuicklistCursor::Gap(gap - usize::from(index < gap)),
            })
        };
        Some(entry)
    }

    /// Returns the stored quicklist entries.
    pub fn entries(&self) -> &[QuicklistEntry] {
        &self.entries
    }

    /// Changes whenever entries or the current navigation position change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the current quicklist position, if one has been visited.
    pub fn current(&self) -> Option<usize> {
        match self.cursor {
            Some(QuicklistCursor::Entry(index)) => Some(index),
            _ => None,
        }
    }

    /// Sets the current quicklist position.
    pub fn set_current(&mut self, current: Option<usize>) {
        let cursor = current
            .filter(|&index| index < self.entries.len())
            .map(QuicklistCursor::Entry);
        if self.cursor != cursor {
            self.cursor = cursor;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(crate) fn next_entry(
        &self,
        count: usize,
        current_doc_id: DocumentId,
        current_path: Option<&Path>,
        same_file: bool,
    ) -> Option<QuicklistMatch<'_>> {
        self.find_entry(
            Direction::Forward,
            count,
            current_doc_id,
            current_path,
            same_file,
        )
    }

    pub(crate) fn prev_entry(
        &self,
        count: usize,
        current_doc_id: DocumentId,
        current_path: Option<&Path>,
        same_file: bool,
    ) -> Option<QuicklistMatch<'_>> {
        self.find_entry(
            Direction::Backward,
            count,
            current_doc_id,
            current_path,
            same_file,
        )
    }

    fn find_entry(
        &self,
        direction: Direction,
        count: usize,
        current_doc_id: DocumentId,
        current_path: Option<&Path>,
        same_file: bool,
    ) -> Option<QuicklistMatch<'_>> {
        if self.entries.is_empty() {
            return None;
        }

        let matches = |entry: &QuicklistEntry| {
            !same_file
                || match &entry.target {
                    QuicklistTarget::Document(doc_id) => *doc_id == current_doc_id,
                    QuicklistTarget::Path(path) => current_path.is_some_and(|current| {
                        stdx::path::canonicalize(path) == stdx::path::canonicalize(current)
                    }),
                }
        };

        if !self.entries.iter().any(matches) {
            return None;
        }

        let len = self.entries.len();
        let mut index = match (direction, self.cursor) {
            (_, Some(QuicklistCursor::Entry(current))) => current,
            (Direction::Forward, Some(QuicklistCursor::Gap(gap))) => (gap + len - 1) % len,
            (Direction::Backward, Some(QuicklistCursor::Gap(gap))) => gap % len,
            (Direction::Forward, None) => len - 1,
            (Direction::Backward, None) => 0,
        };

        let mut remaining = count.max(1);
        while remaining > 0 {
            for _ in 0..len {
                index = match direction {
                    Direction::Forward => (index + 1) % len,
                    Direction::Backward => (index + len - 1) % len,
                };
                if matches(&self.entries[index]) {
                    remaining -= 1;
                    break;
                }
            }
        }

        Some(QuicklistMatch {
            index,
            entry: &self.entries[index],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Quicklist, QuicklistEntry, QuicklistPosition, QuicklistTarget};
    use crate::DocumentId;
    use editor_core::{Rope, Selection};
    use std::path::PathBuf;

    fn entry(path: &str, line: usize) -> QuicklistEntry {
        QuicklistEntry {
            target: QuicklistTarget::Path(PathBuf::from(path)),
            position: QuicklistPosition::LineRange {
                start: line,
                end: line,
            },
        }
    }

    fn unsaved_entry(id: DocumentId, line: usize) -> QuicklistEntry {
        QuicklistEntry {
            target: QuicklistTarget::Document(id),
            position: QuicklistPosition::LineRange {
                start: line,
                end: line,
            },
        }
    }

    #[test]
    fn quicklist_selection_reports_line_range() {
        let text = Rope::from("alpha\nbeta\ngamma\n");
        let position = QuicklistPosition::Selection(Selection::single(6, 10));

        assert_eq!(position.line_range(Some(text.slice(..))), Some((1, 1)));
    }

    #[test]
    fn appending_preserves_entries_and_current_position() {
        let mut quicklist = Quicklist::default();
        quicklist.push(entry("a.rs", 0));
        assert_eq!(quicklist.current(), None);
        quicklist.set_current(Some(0));
        quicklist.push(entry("b.rs", 1));
        assert_eq!(quicklist.entries(), &[entry("a.rs", 0), entry("b.rs", 1)]);
        assert_eq!(quicklist.current(), Some(0));
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), None, false)
                .unwrap()
                .entry,
            &entry("b.rs", 1)
        );
    }

    #[test]
    fn removing_current_continues_from_the_gap_in_both_directions() {
        for (removed, next, prev) in [(0, 1, 3), (1, 2, 0), (2, 3, 1), (3, 0, 2)] {
            let mut quicklist = Quicklist::default();
            quicklist.replace((0..4).map(|line| entry("a.rs", line)).collect());
            quicklist.set_current(Some(removed));
            assert_eq!(quicklist.remove(removed), Some(entry("a.rs", removed)));
            assert_eq!(quicklist.current(), None);
            assert_eq!(
                quicklist
                    .next_entry(1, DocumentId::default(), None, false)
                    .unwrap()
                    .entry,
                &entry("a.rs", next)
            );
            assert_eq!(
                quicklist
                    .prev_entry(1, DocumentId::default(), None, false)
                    .unwrap()
                    .entry,
                &entry("a.rs", prev)
            );
        }
    }

    #[test]
    fn removal_preserves_a_surviving_current_entry() {
        for (removed, current) in [(0, 0), (2, 1), (3, 1)] {
            let mut quicklist = Quicklist::default();
            quicklist.replace((0..4).map(|line| entry("a.rs", line)).collect());
            quicklist.set_current(Some(1));
            quicklist.remove(removed);
            assert_eq!(quicklist.current(), Some(current));
            assert_eq!(quicklist.entries()[current], entry("a.rs", 1));
        }
    }

    #[test]
    fn removing_neighbours_preserves_the_navigation_gap() {
        let mut quicklist = Quicklist::default();
        quicklist.replace((0..5).map(|line| entry("a.rs", line)).collect());
        quicklist.set_current(Some(2));
        quicklist.remove(2);
        quicklist.remove(0);
        quicklist.remove(2);
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), None, false)
                .unwrap()
                .entry,
            &entry("a.rs", 3)
        );
        assert_eq!(
            quicklist
                .prev_entry(1, DocumentId::default(), None, false)
                .unwrap()
                .entry,
            &entry("a.rs", 1)
        );
        quicklist.push(entry("b.rs", 0));
        assert_eq!(
            quicklist
                .next_entry(2, DocumentId::default(), None, false)
                .unwrap()
                .entry,
            &entry("b.rs", 0)
        );
    }

    #[test]
    fn gap_navigation_respects_file_filtering_and_counts() {
        let mut quicklist = Quicklist::default();
        quicklist.replace(vec![
            entry("a.rs", 0),
            entry("b.rs", 1),
            entry("a.rs", 2),
            entry("a.rs", 3),
        ]);
        quicklist.set_current(Some(2));
        quicklist.remove(2);
        let path = PathBuf::from("a.rs");
        assert_eq!(
            quicklist
                .next_entry(2, DocumentId::default(), Some(&path), true)
                .unwrap()
                .entry,
            &entry("a.rs", 0)
        );
        assert_eq!(
            quicklist
                .prev_entry(1, DocumentId::default(), Some(&path), true)
                .unwrap()
                .entry,
            &entry("a.rs", 0)
        );
    }

    #[test]
    fn removing_final_entry_and_replacing_reset_navigation() {
        let mut quicklist = Quicklist::default();
        assert_eq!(quicklist.remove(0), None);
        quicklist.push(entry("a.rs", 0));
        quicklist.set_current(Some(0));
        assert_eq!(quicklist.remove(1), None);
        assert_eq!(quicklist.current(), Some(0));
        quicklist.remove(0);
        assert!(quicklist.entries().is_empty());
        assert_eq!(quicklist.current(), None);
        assert!(quicklist
            .next_entry(1, DocumentId::default(), None, false)
            .is_none());
        quicklist.push(entry("b.rs", 0));
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), None, false)
                .unwrap()
                .index,
            0
        );
        quicklist.set_current(Some(0));
        quicklist.push(entry("c.rs", 0));
        quicklist.remove(0);
        quicklist.replace(vec![entry("d.rs", 0), entry("e.rs", 0)]);
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), None, false)
                .unwrap()
                .entry,
            &entry("d.rs", 0)
        );
    }

    #[test]
    fn quicklist_wraps_globally() {
        let mut quicklist = Quicklist::default();
        quicklist.replace(vec![entry("a.rs", 0), entry("b.rs", 1), entry("c.rs", 2)]);

        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), None, false)
                .map(|entry| entry.index),
            Some(0),
        );
        quicklist.set_current(Some(0));
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), None, false)
                .map(|entry| entry.index),
            Some(1),
        );
        assert_eq!(
            quicklist
                .prev_entry(1, DocumentId::default(), None, false)
                .map(|entry| entry.index),
            Some(2),
        );
    }

    #[test]
    fn quicklist_filters_to_current_file() {
        let mut quicklist = Quicklist::default();
        quicklist.replace(vec![
            entry("a.rs", 0),
            entry("b.rs", 1),
            entry("a.rs", 2),
            entry("c.rs", 3),
        ]);

        let path = PathBuf::from("a.rs");
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), Some(&path), true)
                .map(|entry| entry.index),
            Some(0),
        );
        quicklist.set_current(Some(0));
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), Some(&path), true)
                .map(|entry| entry.index),
            Some(2),
        );
        assert_eq!(
            quicklist
                .prev_entry(1, DocumentId::default(), Some(&path), true)
                .map(|entry| entry.index),
            Some(2),
        );
    }

    #[test]
    fn quicklist_filters_to_current_file_with_canonicalized_entry_path() {
        let mut quicklist = Quicklist::default();
        quicklist.replace(vec![
            entry("./a.rs", 0),
            entry("b.rs", 1),
            entry("dir/../a.rs", 2),
        ]);

        let path = stdx::path::canonicalize("a.rs");
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), Some(&path), true)
                .map(|entry| entry.index),
            Some(0),
        );
        quicklist.set_current(Some(0));
        assert_eq!(
            quicklist
                .next_entry(1, DocumentId::default(), Some(&path), true)
                .map(|entry| entry.index),
            Some(2),
        );
    }

    #[test]
    fn quicklist_filters_to_unsaved_document() {
        let mut quicklist = Quicklist::default();
        let doc_id = DocumentId::default();
        quicklist.replace(vec![
            entry("a.rs", 0),
            unsaved_entry(doc_id, 1),
            entry("b.rs", 2),
        ]);

        assert_eq!(
            quicklist
                .next_entry(1, doc_id, None, true)
                .map(|entry| entry.index),
            Some(1),
        );
        quicklist.set_current(Some(1));
        assert_eq!(
            quicklist
                .prev_entry(1, doc_id, None, true)
                .map(|entry| entry.index),
            Some(1),
        );
    }
}
