//! Editing operations over documents and their per-view selections.

use std::collections::HashMap;

use editor_core::{
    history::{HistoryDirection, HistoryStep, UndoKind},
    line_ending::normalize_line_endings,
    Assoc, Range, Selection, Tendril, Transaction,
};

use crate::{Document, DocumentId, View, ViewId};
use buffer::HistoryAction;

/// Rejections leave both document text and pending undo groups untouched.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    #[error("Document {0:?} is closed")]
    DocumentClosed(DocumentId),
    #[error("Document changed: expected version {expected}, found {actual}")]
    DocumentChanged { expected: i32, actual: i32 },
    #[error("Changes require {actual} characters, but the document has {expected}")]
    InvalidLength { expected: usize, actual: usize },
    #[error("Cannot edit binary document text")]
    BinaryDocument,
}

impl From<buffer::InvalidLength> for EditError {
    fn from(error: buffer::InvalidLength) -> Self {
        Self::InvalidLength {
            expected: error.expected,
            actual: error.actual,
        }
    }
}

/// Cursor snapshots are independent of text history and keyed by its revision IDs.
/// They are shared by the document's views so undo in another split preserves Mitos's
/// existing cursor-restoration behavior, including when the original split closes.
#[derive(Debug, Default)]
pub(crate) struct SelectionHistory {
    pending: Option<(ViewId, Selection)>,
    revisions: HashMap<usize, TransactionSelections>,
}

#[derive(Debug)]
struct TransactionSelections {
    undo: Selection,
    redo: Option<Selection>,
}

impl SelectionHistory {
    pub(crate) fn commit(
        &mut self,
        revision: usize,
        selection: impl FnOnce(ViewId) -> Option<Selection>,
    ) {
        if let Some((origin, undo)) = self.pending.take() {
            self.revisions.insert(
                revision,
                TransactionSelections {
                    undo,
                    redo: selection(origin),
                },
            );
        }
    }

    pub(crate) fn selection(&self, step: &HistoryStep) -> Option<&Selection> {
        let selections = self.revisions.get(&step.revision)?;
        match step.direction {
            HistoryDirection::Undo => Some(&selections.undo),
            HistoryDirection::Redo => selections.redo.as_ref(),
        }
    }
}

/// Apply an interactive transaction with an originating selection. Text mutation
/// and pending undo groups use the same document path as edits without a view.
pub fn apply(doc: &mut Document, transaction: &Transaction, origin: ViewId) -> bool {
    apply_with_notifications(doc, transaction, origin, true)
}

/// Apply a completion preview without sending its temporary text to language servers.
pub fn apply_temporary(doc: &mut Document, transaction: &Transaction, origin: ViewId) -> bool {
    apply_with_notifications(doc, transaction, origin, false)
}

pub(crate) fn apply_with_notifications(
    doc: &mut Document,
    transaction: &Transaction,
    origin: ViewId,
    notify: bool,
) -> bool {
    if doc.validate_changes(transaction.changes()).is_err() {
        return false;
    }
    if doc.changes().is_empty() && !transaction.changes().is_empty() {
        doc.selection_history.pending = Some((origin, doc.selection(origin).clone()));
    }
    doc.apply_inner(transaction, Some(origin), notify).is_ok()
}

/// End an interactive undo group and synchronize this split's navigation state.
pub fn commit(doc: &mut Document, view: &mut View) {
    doc.commit_changes();
    view.sync_changes(doc);
}

fn change_history(doc: &mut Document, view: &mut View, action: HistoryAction) -> bool {
    let applied = doc.change_history(action, Some(view.id));
    if applied {
        view.sync_changes(doc);
    }
    applied
}

pub fn undo(doc: &mut Document, view: &mut View) -> bool {
    change_history(doc, view, HistoryAction::Undo)
}

pub fn redo(doc: &mut Document, view: &mut View) -> bool {
    change_history(doc, view, HistoryAction::Redo)
}

pub fn earlier(doc: &mut Document, view: &mut View, kind: UndoKind) -> bool {
    change_history(doc, view, HistoryAction::Earlier(kind))
}

pub fn later(doc: &mut Document, view: &mut View, kind: UndoKind) -> bool {
    change_history(doc, view, HistoryAction::Later(kind))
}

/// Prefer the last edit at the original cursor, falling back to the first change
/// for operations without a cursor snapshot.
pub fn last_edit_pos(doc: &mut Document) -> Option<usize> {
    let transaction = doc.buffer.current_transaction()?;
    let at_cursor = doc
        .selection_history
        .revisions
        .get(&doc.buffer.current_revision())
        .and_then(|selections| {
            let primary = selections.undo.primary();
            transaction
                .changes_iter()
                .find(|(from, to, _)| Range::new(*from, *to).overlaps(&primary))
        });
    let (_, to, _) = at_cursor.or_else(|| transaction.changes_iter().next())?;
    Some(transaction.changes().map_pos(to, Assoc::After))
}

/// Replace every selection with the same literal text and commit pending edits to history.
///
/// Normalizes replacement line endings to the document's convention and selects
/// each inserted range, preserving selection direction and the primary selection.
/// Positions are character indices. The view must display this document and have
/// an initialized selection. Mode changes, scrolling, prompts, and repeat recording
/// belong to the caller.
pub fn replace_selections(doc: &mut Document, view: &mut View, replacement: &str) {
    let replacement = Tendril::from(normalize_line_endings(replacement, doc.line_ending).as_ref());
    let len = replacement.chars().count();
    let transaction =
        Transaction::change_by_and_with_selection(doc.text(), doc.selection(view.id), |range| {
            let selection =
                Range::new(range.from(), range.from() + len).with_direction(range.direction());
            (
                (range.from(), range.to(), Some(replacement.clone())),
                Some(selection),
            )
        });
    crate::editing::apply(doc, &transaction, view.id);
    crate::editing::commit(doc, view);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use editor_core::{test, LineEnding, Rope};
    use language::syntax;

    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn text_edits_and_history_work_without_any_view() {
        let mut doc = Document::from(
            Rope::from("abc def\n"),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let edit = Transaction::change(
            doc.text(),
            [(0, 1, Some("XY".into())), (4, 5, Some("ZZ".into()))].into_iter(),
        );
        doc.apply(edit.changes()).unwrap();
        doc.commit_changes();
        assert_eq!(last_edit_pos(&mut doc), Some(2));
        assert!(doc.undo());
        assert_eq!(doc.text(), "abc def\n");
        assert!(doc.redo());
        assert_eq!(doc.text(), "XYbc ZZef\n");
        assert!(doc.selections().next().is_none());
    }

    #[tokio::test]
    async fn last_edit_prefers_the_change_at_the_original_selection() {
        let (mut doc, mut view) = document_and_view("abc #[d|]#ef\n");
        let edit = Transaction::change(
            doc.text(),
            [(0, 1, Some("XY".into())), (4, 5, Some("ZZ".into()))].into_iter(),
        );
        assert!(apply(&mut doc, &edit, view.id));
        commit(&mut doc, &mut view);
        assert_eq!(last_edit_pos(&mut doc), Some(7));
    }

    fn document_and_view(input: &str) -> (Document, View) {
        let (text, selection) = test::print(input);
        let mut doc = Document::from(
            Rope::from(text),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let view = View::new(doc.id(), Default::default(), event::Redraw::default());
        doc.ensure_view_init(view.id);
        doc.set_selection(view.id, selection);
        (doc, view)
    }

    #[tokio::test]
    async fn replacement_preserves_selections_and_undo_without_a_frontend() {
        for (input, replacement, line_ending, expected) in [
            (
                "#(e\u{301}|)# #[|界]#\n",
                "é界$1(",
                LineEnding::LF,
                "#(é界$1(|)# #[|é界$1(]#\n",
            ),
            (
                "#[foo|]# #(bar|)#\r\n",
                "one\ntwo\rthree\r\nfour",
                LineEnding::Crlf,
                "#[one\r\ntwo\r\nthree\r\nfour|]# #(one\r\ntwo\r\nthree\r\nfour|)#\r\n",
            ),
            ("foo#[|]#", "x", LineEnding::LF, "foo#[x|]#"),
            ("#[abc|]#\n", "", LineEnding::LF, "#[\n|]#"),
            ("#[|]#", "界", LineEnding::LF, "#[界|]#"),
        ] {
            let (mut doc, mut view) = document_and_view(input);
            doc.line_ending = line_ending;
            let original_text = doc.text().clone();
            let original_selection = doc.selection(view.id).clone();
            let version = doc.version();

            replace_selections(&mut doc, &mut view, replacement);

            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
            assert_eq!(doc.version(), version + 1);
            assert!(crate::editing::undo(&mut doc, &mut view));
            assert_eq!(doc.text(), &original_text);
            assert_eq!(doc.selection(view.id), &original_selection);
            assert!(!crate::editing::undo(&mut doc, &mut view));
            assert!(crate::editing::redo(&mut doc, &mut view));
            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
        }
    }

    #[tokio::test]
    async fn successive_replacements_have_separate_history_checkpoints() {
        let (mut doc, mut view) = document_and_view("#[original|]#\n");

        replace_selections(&mut doc, &mut view, "first");
        replace_selections(&mut doc, &mut view, "second");

        for expected in ["#[first|]#\n", "#[original|]#\n"] {
            assert!(crate::editing::undo(&mut doc, &mut view));
            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
        }
        for expected in ["#[first|]#\n", "#[second|]#\n"] {
            assert!(crate::editing::redo(&mut doc, &mut view));
            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
        }
    }
}
