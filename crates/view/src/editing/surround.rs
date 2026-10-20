//! Surround edits and the document-bound preview used by two-step replacement.

use super::EditError;
use crate::{Document, DocumentId, ViewId};
use editor_core::{match_brackets::get_pair, Range, Selection, SmallVec, Tendril, Transaction};
use language::surround;

#[derive(Clone, Copy)]
pub enum Delimiter {
    Character(char),
    LineEnding,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("{0}")]
    Pair(surround::Error),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error("Surround selection changed; start the command again")]
    StaleSelection,
}

/// Surround every selection, preserving direction and the primary selection.
pub fn add(doc: &mut Document, view: ViewId, delimiter: Delimiter) -> Result<(), EditError> {
    let (open, close): (Tendril, Tendril) = match delimiter {
        Delimiter::Character(ch) => {
            let (open, close) = get_pair(ch);
            (open.to_string().into(), close.to_string().into())
        }
        Delimiter::LineEnding => (
            doc.line_ending.as_str().into(),
            doc.line_ending.as_str().into(),
        ),
    };
    let added = open.chars().count() + close.chars().count();
    let selection = doc.selection(view);
    let mut changes = Vec::with_capacity(selection.len() * 2);
    let mut ranges = SmallVec::with_capacity(selection.len());
    let mut offset = 0;
    for range in selection {
        changes.push((range.from(), range.from(), Some(open.clone())));
        changes.push((range.to(), range.to(), Some(close.clone())));
        ranges.push(
            Range::new(offset + range.from(), offset + range.to() + added)
                .with_direction(range.direction()),
        );
        offset += added;
    }
    let transaction = Transaction::change(doc.text(), changes.into_iter())
        .with_selection(Selection::new(ranges, selection.primary_index()));
    super::try_apply(doc, &transaction, view)
}

pub fn delete(
    doc: &mut Document,
    view: ViewId,
    character: Option<char>,
    count: usize,
) -> Result<(), Error> {
    let mut positions = surround::get_surround_pos(
        doc.syntax(),
        doc.text().slice(..),
        doc.selection(view),
        character,
        count,
    )
    .map_err(Error::Pair)?;
    positions.sort_unstable();
    let transaction = Transaction::change(
        doc.text(),
        positions.into_iter().map(|pos| (pos, pos + 1, None)),
    );
    super::try_apply(doc, &transaction, view)?;
    Ok(())
}

/// A replacement preview belongs to one document/view and one unchanged text snapshot.
/// Dropping a stale preview never restores its old selection onto newer editor state.
pub struct Replacement {
    document: DocumentId,
    view: ViewId,
    snapshot: buffer::Snapshot,
    original: Selection,
    preview: Selection,
    positions: Vec<usize>,
}

impl Replacement {
    /// Select the surround characters while the frontend requests a replacement pair.
    pub fn prepare(
        doc: &mut Document,
        view: ViewId,
        character: Option<char>,
        count: usize,
    ) -> Result<Self, Error> {
        let original = doc.selection(view).clone();
        let positions = surround::get_surround_pos(
            doc.syntax(),
            doc.text().slice(..),
            &original,
            character,
            count,
        )
        .map_err(Error::Pair)?;
        let preview = Selection::new(
            positions.iter().map(|&pos| Range::point(pos)).collect(),
            original.primary_index() * 2,
        )
        .ensure_invariants(doc.text().slice(..));
        let replacement = Self {
            document: doc.id(),
            view,
            snapshot: doc.snapshot(),
            original,
            preview: preview.clone(),
            positions,
        };
        doc.set_selection(view, preview);
        Ok(replacement)
    }

    fn is_current(&self, doc: &Document, view: ViewId) -> bool {
        self.document == doc.id()
            && self.view == view
            && self.snapshot.is_current(&doc.buffer)
            && doc.try_selection(view) == Some(&self.preview)
    }

    /// Cancel only while the original text and preview selection are still current.
    pub fn cancel(self, doc: &mut Document, view: ViewId) {
        if self.is_current(doc, view) {
            doc.set_selection(view, self.original);
        }
    }

    pub fn apply(self, doc: &mut Document, view: ViewId, character: char) -> Result<(), Error> {
        if !self.is_current(doc, view) {
            return Err(Error::StaleSelection);
        }
        let (open, close) = get_pair(character);
        let mut changes: Vec<_> = self
            .positions
            .chunks_exact(2)
            .flat_map(|pair| [(pair[0], open), (pair[1], close)])
            .collect();
        changes.sort_unstable();
        let transaction = Transaction::change(
            doc.text(),
            changes
                .into_iter()
                .map(|(pos, ch)| (pos, pos + 1, Some(ch.to_string().into()))),
        );
        if let Err(error) = doc.validate_changes(transaction.changes()) {
            self.cancel(doc, view);
            return Err(error.into());
        }
        doc.set_selection(view, self.original.clone());
        // Selection observers can edit the document or redirect the selection.
        if !self.snapshot.is_current(&doc.buffer) || doc.try_selection(view) != Some(&self.original)
        {
            return Err(Error::StaleSelection);
        }
        super::try_apply(doc, &transaction, view)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::{commit, tests::document_and_view, undo};
    use editor_core::{test, LineEnding};

    #[tokio::test]
    async fn surrounding_unicode_and_crlf_preserves_selections_and_undo() {
        for (input, delimiter, expected) in [
            (
                "#[|界]# #(é|)#",
                Delimiter::Character('('),
                "#[|(界)]# #((é)|)#",
            ),
            ("#[|界]#", Delimiter::LineEnding, "#[|\r\n界\r\n]#"),
        ] {
            let (mut doc, mut view) = document_and_view(input);
            doc.line_ending = LineEnding::Crlf;
            let original = doc.selection(view.id).clone();
            add(&mut doc, view.id, delimiter).unwrap();
            let (text, selection) = test::print(expected);
            assert_eq!(doc.text(), text.as_str());
            assert_eq!(doc.selection(view.id), &selection);
            commit(&mut doc, &mut view);
            assert!(undo(&mut doc, &mut view));
            assert_eq!(doc.selection(view.id), &original);
        }
    }

    #[tokio::test]
    async fn replacement_restores_the_original_selection_for_undo_and_cancellation() {
        let (mut doc, mut view) = document_and_view("(#[|é]#)\n");
        let original = doc.selection(view.id).clone();
        let replacement = Replacement::prepare(&mut doc, view.id, Some('('), 1).unwrap();
        replacement.cancel(&mut doc, view.id);
        assert_eq!(doc.selection(view.id), &original);
        let replacement = Replacement::prepare(&mut doc, view.id, Some('('), 1).unwrap();
        replacement.apply(&mut doc, view.id, '[').unwrap();
        assert_eq!(doc.text(), "[é]\n");
        commit(&mut doc, &mut view);
        assert!(undo(&mut doc, &mut view));
        assert_eq!(doc.text(), "(é)\n");
        assert_eq!(doc.selection(view.id), &original);
    }

    #[tokio::test]
    async fn changed_text_cannot_apply_or_cancel_a_surround_preview() {
        for apply in [false, true] {
            let (mut doc, view) = document_and_view("(#[|é]#)\n");
            let replacement = Replacement::prepare(&mut doc, view.id, Some('('), 1).unwrap();
            let edit = Transaction::change(doc.text(), [(0, 0, Some("x".into()))].into_iter());
            doc.apply(edit.changes()).unwrap();
            let selection = doc.selection(view.id).clone();
            if apply {
                assert_eq!(
                    replacement.apply(&mut doc, view.id, '['),
                    Err(Error::StaleSelection)
                );
            } else {
                replacement.cancel(&mut doc, view.id);
            }
            assert_eq!(doc.text(), "x(é)\n");
            assert_eq!(doc.selection(view.id), &selection);
        }
    }

    #[tokio::test]
    async fn previews_cannot_cross_documents_views_or_selection_changes() {
        let (mut doc, view) = document_and_view("(#[|é]#)\n");
        let replacement = Replacement::prepare(&mut doc, view.id, None, 1).unwrap();
        let (mut other, other_view) = document_and_view("(#[|é]#)\n");
        let selection = other.selection(other_view.id).clone();
        assert_eq!(
            replacement.apply(&mut other, other_view.id, '['),
            Err(Error::StaleSelection)
        );
        assert_eq!(other.selection(other_view.id), &selection);

        let (mut doc, view) = document_and_view("(#[|é]#)\n");
        let replacement = Replacement::prepare(&mut doc, view.id, None, 1).unwrap();
        doc.set_selection(view.id, Selection::point(1));
        let selection = doc.selection(view.id).clone();
        replacement.cancel(&mut doc, view.id);
        assert_eq!(doc.selection(view.id), &selection);
    }

    #[tokio::test]
    async fn replacement_rejects_a_different_or_removed_view() {
        let (mut doc, view) = document_and_view("(#[|a]#)\n");
        let mut ids = slotmap::SlotMap::<ViewId, ()>::with_key();
        let other = ids.insert(());
        doc.set_selection(other, doc.selection(view.id).clone());
        let replacement = Replacement::prepare(&mut doc, view.id, None, 1).unwrap();
        assert_eq!(
            replacement.apply(&mut doc, other, '['),
            Err(Error::StaleSelection)
        );
        doc.set_selection(view.id, Selection::point(1));
        let replacement = Replacement::prepare(&mut doc, view.id, None, 1).unwrap();
        doc.remove_view(view.id);
        assert_eq!(
            replacement.apply(&mut doc, view.id, '['),
            Err(Error::StaleSelection)
        );
        assert!(doc.try_selection(view.id).is_none());
    }

    #[tokio::test]
    async fn rejected_replacements_restore_the_selection_without_recording_an_edit() {
        let (doc, mut view) = document_and_view("(#[|a]#)\n");
        let mut doc = doc.with_readonly_name("adapter source".into());
        let original = doc.selection(view.id).clone();
        let replacement = Replacement::prepare(&mut doc, view.id, None, 1).unwrap();
        assert_eq!(
            replacement.apply(&mut doc, view.id, '['),
            Err(Error::Edit(EditError::ReadOnlyDocument))
        );
        assert_eq!(doc.selection(view.id), &original);
        assert_eq!(doc.text(), "(a)\n");
        assert!(!undo(&mut doc, &mut view));
    }

    #[tokio::test]
    async fn overlapping_pairs_are_rejected_without_changing_text_or_selections() {
        let (mut doc, view) = document_and_view("(#[|a]#x#(b|)#)\n");
        let original = doc.selection(view.id).clone();
        assert_eq!(
            delete(&mut doc, view.id, None, 1),
            Err(Error::Pair(surround::Error::CursorOverlap))
        );
        assert_eq!(doc.text(), "(axb)\n");
        assert_eq!(doc.selection(view.id), &original);
    }
}
