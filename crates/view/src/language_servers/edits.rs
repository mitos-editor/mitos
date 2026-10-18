//! Translate protocol diagnostics, completion payloads, and text edits into editor values.

use editor_core::{chars, Rope, RopeSlice, Selection, Tendril, Transaction};
use lsp_client::{lsp, util::lsp_pos_to_pos, OffsetEncoding};
use snippets::{RenderedSnippet, Snippet, SnippetRenderCtx};

/// If the LS did not provide a range for the completion or the range of the
/// primary cursor can not be used for the secondary cursor, this function
/// can be used to find the completion range for a cursor
fn find_completion_range(text: RopeSlice, replace_mode: bool, cursor: usize) -> (usize, usize) {
    let start = cursor
        - text
            .chars_at(cursor)
            .reversed()
            .take_while(|ch| chars::char_is_word(*ch))
            .count();
    let mut end = cursor;
    if replace_mode {
        end += text
            .chars_at(cursor)
            .take_while(|ch| chars::char_is_word(*ch))
            .count();
    }
    (start, end)
}
fn completion_range(
    text: RopeSlice,
    edit_offset: Option<(i128, i128)>,
    replace_mode: bool,
    cursor: usize,
) -> Option<(usize, usize)> {
    let res = match edit_offset {
        Some((start_offset, end_offset)) => {
            let start_offset = cursor as i128 + start_offset;
            if start_offset < 0 {
                return None;
            }
            let end_offset = cursor as i128 + end_offset;
            if end_offset > text.len_chars() as i128 {
                return None;
            }
            (start_offset as usize, end_offset as usize)
        }
        None => find_completion_range(text, replace_mode, cursor),
    };
    Some(res)
}

/// Creates a [Transaction] from the [lsp::TextEdit] in a completion response.
/// The transaction applies the edit to all cursors.
pub fn generate_transaction_from_completion_edit(
    doc: &Rope,
    selection: &Selection,
    edit_offset: Option<(i128, i128)>,
    replace_mode: bool,
    new_text: String,
) -> Transaction {
    let replacement: Option<Tendril> = if new_text.is_empty() {
        None
    } else {
        Some(new_text.into())
    };

    let text = doc.slice(..);
    let (removed_start, removed_end) = completion_range(
        text,
        edit_offset,
        replace_mode,
        selection.primary().cursor(text),
    )
    .expect("transaction must be valid for primary selection");
    let removed_text = text.slice(removed_start..removed_end);

    let (transaction, mut selection) = Transaction::change_by_selection_ignore_overlapping(
        doc,
        selection,
        |range| {
            let cursor = range.cursor(text);
            completion_range(text, edit_offset, replace_mode, cursor)
                .filter(|(start, end)| text.slice(start..end) == removed_text)
                .unwrap_or_else(|| find_completion_range(text, replace_mode, cursor))
        },
        |_, _| replacement.clone(),
    );
    if transaction.changes().is_empty() {
        return transaction;
    }
    selection = selection.map(transaction.changes());
    transaction.with_selection(selection)
}

/// Creates a [Transaction] from the [Snippet] in a completion response.
/// The transaction applies the edit to all cursors.
pub fn generate_transaction_from_snippet(
    doc: &Rope,
    selection: &Selection,
    edit_offset: Option<(i128, i128)>,
    replace_mode: bool,
    snippet: Snippet,
    cx: &mut SnippetRenderCtx,
) -> (Transaction, RenderedSnippet) {
    let text = doc.slice(..);
    let (removed_start, removed_end) = completion_range(
        text,
        edit_offset,
        replace_mode,
        selection.primary().cursor(text),
    )
    .expect("transaction must be valid for primary selection");
    let removed_text = text.slice(removed_start..removed_end);
    let (transaction, mapped_selection, snippet) = snippet.render(
        doc,
        selection,
        |range| {
            let cursor = range.cursor(text);
            completion_range(text, edit_offset, replace_mode, cursor)
                .filter(|(start, end)| text.slice(start..end) == removed_text)
                .unwrap_or_else(|| find_completion_range(text, replace_mode, cursor))
        },
        cx,
    );
    let transaction = transaction.with_selection(snippet.first_selection(
        // we keep the direction of the old primary selection in case it changed during mapping
        // but use the primary idx from the mapped selection in case ranges had to be merged
        selection.primary().direction(),
        mapped_selection.primary_index(),
    ));
    (transaction, snippet)
}

pub fn generate_transaction_from_edits(
    doc: &Rope,
    mut edits: Vec<lsp::TextEdit>,
    offset_encoding: OffsetEncoding,
) -> Transaction {
    // Sort edits by start range, since some LSPs (Omnisharp) send them
    // in reverse order.
    edits.sort_by_key(|edit| edit.range.start);

    // Generate a diff if the edit is a full document replacement.
    #[allow(clippy::collapsible_if)]
    if edits.len() == 1 {
        let is_document_replacement = edits.first().and_then(|edit| {
            let start = lsp_pos_to_pos(doc, edit.range.start, offset_encoding)?;
            let end = lsp_pos_to_pos(doc, edit.range.end, offset_encoding)?;
            Some(start..end)
        }) == Some(0..doc.len_chars());
        if is_document_replacement {
            let new_text = Rope::from(edits.pop().unwrap().new_text);
            return text_diff::compare_ropes(doc, &new_text);
        }
    }

    // `ChangeSet::from_changes` requires its changes to be sorted and
    // non-overlapping (it does `retain(from - last)`, which underflows when
    // `from < last`). The sort above handles ordering, but the LSP spec
    // already forbids overlapping edits and some servers violate that. So
    // resolve the edits here and drop any that overlap an earlier one (or
    // fail to map), instead of feeding an overlap into `from_changes` and
    // panicking. This matches the overlap policy of `Transaction::delete`.
    // See issue #15514 / AUDIT-044/045.
    let mut last_end = 0;
    let mut changes: Vec<(usize, usize, Option<editor_core::Tendril>)> =
        Vec::with_capacity(edits.len());
    for edit in edits {
        let Some(start) = lsp_pos_to_pos(doc, edit.range.start, offset_encoding) else {
            continue;
        };
        let Some(end) = lsp_pos_to_pos(doc, edit.range.end, offset_encoding) else {
            continue;
        };

        if start > end {
            log::error!("Invalid LSP text edit start {start:?} > end {end:?}, discarding");
            continue;
        }

        if start < last_end {
            log::error!("Overlapping LSP text edit {start}..{end} (after {last_end}), discarding");
            continue;
        }
        last_end = end;

        // simplify "" into None for cleaner changesets
        let replacement = if edit.new_text.is_empty() {
            None
        } else {
            Some(edit.new_text.into())
        };

        changes.push((start, end, replacement));
    }

    Transaction::change(doc, changes.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emoji_format_gh_4791() {
        use lsp::{Position, Range, TextEdit};

        let edits = vec![
            TextEdit {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 1,
                    },
                    end: Position {
                        line: 1,
                        character: 0,
                    },
                },
                new_text: "\n  ".to_string(),
            },
            TextEdit {
                range: Range {
                    start: Position {
                        line: 1,
                        character: 7,
                    },
                    end: Position {
                        line: 2,
                        character: 0,
                    },
                },
                new_text: "\n  ".to_string(),
            },
        ];

        let mut source = Rope::from_str("[\n\"🇺🇸\",\n\"🎄\",\n]");

        let transaction = generate_transaction_from_edits(&source, edits, OffsetEncoding::Utf16);
        assert!(transaction.apply(&mut source));
        assert_eq!(source, "[\n  \"🇺🇸\",\n  \"🎄\",\n]");
    }

    #[test]
    fn overlapping_edits_are_dropped() {
        // Regression for issue #15514: a language server may send overlapping text edits.
        // Feeding overlapping ranges to `ChangeSet::from_changes` violates its sorted/non-overlapping
        // precondition and underflows `retain(from - last)`. The overlapping edit must be discarded here
        use lsp::{Position, Range, TextEdit};

        let edit = |sc, ec, text: &str| TextEdit {
            range: Range {
                start: Position::new(0, sc),
                end: Position::new(0, ec),
            },
            new_text: text.to_string(),
        };

        // Edits are given out of order and the second overlaps the first
        // (0..3 vs 2..4); after sorting, 2..4 starts before 0..3 ends.
        let edits = vec![edit(4, 5, "Z"), edit(0, 3, "X"), edit(2, 4, "Y")];

        let mut source = Rope::from_str("abcdef");
        let transaction = generate_transaction_from_edits(&source, edits, OffsetEncoding::Utf16);
        // Must not panic. The overlapping 2..4 edit is dropped; 0..3 -> "X",
        // retain "d" (idx 3), 4..5 -> "Z", retain "f": "abcdef" -> "XdZf".
        assert!(transaction.apply(&mut source));
        assert_eq!(source, "XdZf");
    }

    #[test]
    fn unsorted_edits_are_applied() {
        // Out-of-order (but non-overlapping) edits must all land; the sort keeps
        // every edit rather than dropping the earlier-positioned one.
        use lsp::{Position, Range, TextEdit};

        let edit = |sc, ec, text: &str| TextEdit {
            range: Range {
                start: Position::new(0, sc),
                end: Position::new(0, ec),
            },
            new_text: text.to_string(),
        };

        let edits = vec![edit(4, 5, "Y"), edit(0, 1, "X")];

        let mut source = Rope::from_str("abcdef");
        let transaction = generate_transaction_from_edits(&source, edits, OffsetEncoding::Utf16);
        assert!(transaction.apply(&mut source));
        assert_eq!(source, "XbcdYf");
    }
}
