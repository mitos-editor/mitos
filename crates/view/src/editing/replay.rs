//! Checked replay of recorded completion edits in character coordinates.

use editor_core::{Change, Rope, Selection, Transaction};

use crate::{document::SavePoint, features::completion::CompletionApply, Document, ViewId};

use super::EditError;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error("Recorded edit cannot be applied at this cursor")]
    InvalidEdit,
    #[error("Recorded cursor is outside the resulting text")]
    InvalidSelection,
    #[error("Cannot repeat completion with a different cursor arrangement")]
    IncompatibleSelections,
    #[error(transparent)]
    Edit(#[from] EditError),
}

/// Reject immutable text before replay executes its entry command or keys.
pub fn validate_target(doc: &Document) -> Result<(), EditError> {
    doc.validate_editable()
}

fn shift(position: usize, recorded: usize, current: usize) -> Option<usize> {
    if current >= recorded {
        position.checked_add(current - recorded)
    } else {
        position.checked_sub(recorded - current)
    }
}

/// Validate all ranges before constructing a transaction: its constructor assumes
/// sorted, non-overlapping, in-bounds changes. Never clip a recorded replacement
/// that starts before the current buffer or let coordinate arithmetic wrap.
fn transaction(
    text: &Rope,
    recorded_cursor: usize,
    current_cursor: usize,
    changes: &[Change],
    selection: Option<usize>,
) -> Result<Transaction, ReplayError> {
    let mut shifted = Vec::with_capacity(changes.len());
    let mut previous_end = 0;
    let mut result_len = text.len_chars();
    for (from, to, inserted) in changes {
        let from = shift(*from, recorded_cursor, current_cursor).ok_or(ReplayError::InvalidEdit)?;
        let to = shift(*to, recorded_cursor, current_cursor).ok_or(ReplayError::InvalidEdit)?;
        if from < previous_end || from > to || to > text.len_chars() {
            return Err(ReplayError::InvalidEdit);
        }
        result_len = result_len
            .checked_sub(to - from)
            .and_then(|len| {
                len.checked_add(inserted.as_ref().map_or(0, |text| text.chars().count()))
            })
            .ok_or(ReplayError::InvalidEdit)?;
        previous_end = to;
        shifted.push((from, to, inserted.clone()));
    }
    let selection = selection
        .map(|position| {
            shift(position, recorded_cursor, current_cursor)
                .filter(|position| *position <= result_len)
                .ok_or(ReplayError::InvalidSelection)
        })
        .transpose()?;
    let transaction = Transaction::change(text, shifted.into_iter());
    Ok(match selection {
        Some(position) => transaction.with_selection(Selection::point(position)),
        None => transaction,
    })
}

/// Replay an accepted menu completion against its original request checkpoint.
/// Restoration and the recorded edit are validated and applied together, so an
/// invalid edit cannot leave the document rolled back to the request state.
pub fn completion(
    doc: &mut Document,
    view: ViewId,
    savepoint: Option<&SavePoint>,
    applied: &CompletionApply,
) -> Result<(), ReplayError> {
    let revert = savepoint
        .map(|saved| doc.savepoint_transaction(view, saved))
        .transpose()?;
    let mut text = doc.text().clone();
    let selection = if let Some(revert) = &revert {
        revert.apply(&mut text);
        revert.selection().expect("savepoint has a selection")
    } else {
        doc.selection(view)
    };
    let cursor = selection.primary().cursor(text.slice(..));
    // The record describes the whole accepted edit, including all rendered
    // snippet instances. Reject a different cursor layout before touching text.
    if applied.cursors.len() != selection.len()
        || applied
            .cursors
            .iter()
            .zip(selection)
            .any(|(&recorded, range)| {
                shift(recorded, applied.trigger_offset, cursor)
                    != Some(range.cursor(text.slice(..)))
            })
    {
        return Err(ReplayError::IncompatibleSelections);
    }
    let replay = transaction(
        &text,
        applied.trigger_offset,
        cursor,
        &applied.changes,
        None,
    )?;
    let result_len = replay.changes().len_after();
    let position = |pos| {
        shift(pos, applied.trigger_offset, cursor)
            .filter(|&pos| pos <= result_len)
            .ok_or(ReplayError::InvalidSelection)
    };
    let ranges = applied
        .selection
        .iter()
        .map(|range| {
            Ok(editor_core::Range::new(
                position(range.anchor)?,
                position(range.head)?,
            ))
        })
        .collect::<Result<_, ReplayError>>()?;
    let selection = Selection::new(ranges, applied.selection.primary_index());
    let mut snippet = applied.snippet.clone();
    if let Some(snippet) = &mut snippet {
        for range in snippet.ranges.iter_mut().chain(
            snippet
                .tabstops
                .iter_mut()
                .flat_map(|tabstop| &mut tabstop.ranges),
        ) {
            range.start = position(range.start)?;
            range.end = position(range.end)?;
        }
    }
    let replay = replay.with_selection(selection);
    let transaction = match revert {
        Some(revert) => revert.compose(replay),
        None => replay,
    };
    super::apply(doc, &transaction, view)?;
    if let Some(snippet) = snippet {
        doc.install_snippet(view, snippet);
    }
    Ok(())
}

/// Replay an inline completion and its resulting cursor without a server request.
pub fn inline_completion(
    doc: &mut Document,
    view: ViewId,
    applied: &crate::features::inline_completion::AppliedInlineCompletion,
) -> Result<(), ReplayError> {
    let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
    let transaction = transaction(
        doc.text(),
        applied.cursor,
        cursor,
        &applied.changes,
        Some(applied.selection),
    )?;
    super::apply(doc, &transaction, view)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::{self, tests::document_and_view};
    use crate::features::inline_completion::AppliedInlineCompletion;

    #[test]
    fn rebasing_preserves_unicode_character_coordinates_and_the_recorded_cursor() {
        let text = Rope::from("🙂 ab\r\n");
        let replay = transaction(&text, 12, 4, &[(10, 12, Some("界é".into()))], Some(12)).unwrap();
        let mut result = text;
        replay.apply(&mut result);
        assert_eq!(result, "🙂 界é\r\n");
        assert_eq!(replay.selection(), Some(&Selection::point(4)));
        let replay = transaction(
            &result,
            0,
            result.len_chars(),
            &[(0, 0, Some("終".into()))],
            Some(1),
        )
        .unwrap();
        replay.apply(&mut result);
        assert_eq!(result, "🙂 界é\r\n終");
    }

    #[test]
    fn recorded_edits_require_valid_ranges_and_resulting_cursors() {
        let text = Rope::from("abc");
        for (recorded, current, changes) in [
            (3, 0, vec![(0, 3, None)]),
            (0, 1, vec![(usize::MAX, usize::MAX, None)]),
            (0, 0, vec![(2, 1, None)]),
            (0, 0, vec![(0, 4, None)]),
            (0, 0, vec![(1, 3, None), (2, 3, None)]),
            (0, 0, vec![(2, 2, None), (0, 1, None)]),
        ] {
            assert_eq!(
                transaction(&text, recorded, current, &changes, None).unwrap_err(),
                ReplayError::InvalidEdit
            );
        }
        assert_eq!(
            transaction(&text, 0, 0, &[(0, 3, None)], Some(1)).unwrap_err(),
            ReplayError::InvalidSelection
        );
    }

    #[tokio::test]
    async fn invalid_completion_replay_preserves_pending_typing() {
        let (mut doc, view) = document_and_view("ab#[|c]#\n");
        let saved = doc.savepoint(view.id);
        let typing = Transaction::change(doc.text(), [(2, 2, Some("界".into()))].into_iter());
        editing::apply(&mut doc, &typing, view.id).unwrap();
        let before = doc.text().clone();
        let selection = doc.selection(view.id).clone();
        let version = doc.version();
        let pending = doc.changes().clone();
        assert_eq!(
            completion(
                &mut doc,
                view.id,
                Some(&saved),
                &CompletionApply {
                    trigger_offset: 2,
                    cursors: vec![2],
                    changes: vec![(0, 99, None)],
                    selection: Selection::point(2),
                    snippet: None,
                }
            ),
            Err(ReplayError::InvalidEdit)
        );
        assert_eq!(doc.text(), &before);
        assert_eq!(doc.selection(view.id), &selection);
        assert_eq!(doc.version(), version);
        assert_eq!(doc.changes(), &pending);
        completion(
            &mut doc,
            view.id,
            Some(&saved),
            &CompletionApply {
                trigger_offset: 2,
                cursors: vec![2],
                changes: vec![(0, 2, Some("done".into()))],
                selection: Selection::point(4),
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(doc.text(), "donec\n");
        assert_eq!(saved.saved_state(&doc).unwrap().0, "abc\n");
    }

    #[tokio::test]
    async fn completion_rejects_changed_cursor_arrangements_without_restoring_its_checkpoint() {
        let (mut doc, view) = document_and_view("#[|a]#b\n");
        let saved = doc.savepoint(view.id);
        editing::insert::insert_char(&mut doc, view.id, '界', None).unwrap();
        let before = doc.text().clone();
        let selection = doc.selection(view.id).clone();
        let pending = doc.changes().clone();
        let applied = CompletionApply {
            trigger_offset: 0,
            cursors: vec![0, 2],
            changes: vec![(0, 1, Some("x".into()))],
            selection: Selection::point(1),
            snippet: None,
        };
        assert_eq!(
            completion(&mut doc, view.id, Some(&saved), &applied),
            Err(ReplayError::IncompatibleSelections)
        );
        assert_eq!(doc.text(), &before);
        assert_eq!(doc.selection(view.id), &selection);
        assert_eq!(doc.changes(), &pending);
    }

    #[tokio::test]
    async fn rejected_inline_replay_preserves_text_selection_and_history() {
        let (doc, view) = document_and_view("a#[|b]#\n");
        let mut doc = doc.with_readonly_name("adapter source".into());
        let before = doc.text().clone();
        let selection = doc.selection(view.id).clone();
        let applied = AppliedInlineCompletion {
            cursor: 1,
            changes: vec![(1, 2, Some("界".into()))],
            selection: 2,
        };
        assert_eq!(
            inline_completion(&mut doc, view.id, &applied),
            Err(ReplayError::Edit(EditError::ReadOnlyDocument))
        );
        assert_eq!(doc.text(), &before);
        assert_eq!(doc.selection(view.id), &selection);
        assert!(!editing::undo(&mut doc, view.id));
    }
}
