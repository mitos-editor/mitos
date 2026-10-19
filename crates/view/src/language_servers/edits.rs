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

/// A malformed edit, retaining its index in the server's original edit array.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TextEditError {
    #[error("text edit {edit_idx} has a reversed range {range:?}")]
    ReversedRange { edit_idx: usize, range: lsp::Range },
    #[error("text edit {edit_idx} has an invalid position {position:?}")]
    InvalidPosition {
        edit_idx: usize,
        position: lsp::Position,
    },
    #[error("text edit {edit_idx} overlaps text edit {previous_edit_idx}")]
    Overlapping {
        edit_idx: usize,
        previous_edit_idx: usize,
    },
}

/// Validate an entire document edit before constructing its transaction.
/// Unsorted edits are accepted; equal-position inserts retain server order.
pub fn generate_transaction_from_edits(
    doc: &Rope,
    edits: Vec<lsp::TextEdit>,
    offset_encoding: OffsetEncoding,
) -> Result<Transaction, TextEditError> {
    let mut edits: Vec<_> = edits.into_iter().enumerate().collect();
    edits.sort_by_key(|(_, edit)| edit.range.start);
    let mut last_end = 0;
    let mut previous_edit_idx = 0;
    let mut changes: Vec<(usize, usize, Option<editor_core::Tendril>)> =
        Vec::with_capacity(edits.len());
    for (edit_idx, edit) in edits {
        if edit.range.start > edit.range.end {
            return Err(TextEditError::ReversedRange {
                edit_idx,
                range: edit.range,
            });
        }
        let start = edit_position(doc, edit.range.start, offset_encoding, edit_idx)?;
        let end = edit_position(doc, edit.range.end, offset_encoding, edit_idx)?;

        if start < last_end {
            return Err(TextEditError::Overlapping {
                edit_idx,
                previous_edit_idx,
            });
        }
        last_end = end;
        previous_edit_idx = edit_idx;

        // simplify "" into None for cleaner changesets
        let replacement = if edit.new_text.is_empty() {
            None
        } else {
            Some(edit.new_text.into())
        };

        changes.push((start, end, replacement));
    }

    // Diff a validated full-document replacement to preserve unchanged anchors.
    if let [(0, end, replacement)] = changes.as_slice()
        && *end == doc.len_chars()
    {
        let new_text = Rope::from_str(replacement.as_deref().unwrap_or(""));
        return Ok(text_diff::compare_ropes(doc, &new_text));
    }
    Ok(Transaction::change(doc, changes.into_iter()))
}

fn edit_position(
    doc: &Rope,
    position: lsp::Position,
    encoding: OffsetEncoding,
    edit_idx: usize,
) -> Result<usize, TextEditError> {
    let invalid = || TextEditError::InvalidPosition { edit_idx, position };
    let offset = lsp_pos_to_pos(doc, position, encoding).ok_or_else(invalid)?;
    // Preserve protocol end-of-line/EOF clamping, but reject offsets inside a
    // UTF-8 code point or UTF-16 surrogate pair instead of rounding them down.
    let actual = lsp_client::util::pos_to_lsp_pos(doc, offset, encoding);
    if actual.line == position.line
        && actual.character < position.character
        && offset
            != editor_core::line_ending::line_end_char_index(&doc.slice(..), position.line as usize)
    {
        return Err(invalid());
    }
    Ok(offset)
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

        let transaction =
            generate_transaction_from_edits(&source, edits, OffsetEncoding::Utf16).unwrap();
        assert!(transaction.apply(&mut source));
        assert_eq!(source, "[\n  \"🇺🇸\",\n  \"🎄\",\n]");
    }

    #[test]
    fn overlapping_edits_reject_the_document_change() {
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

        let source = Rope::from_str("abcdef");
        assert!(matches!(
            generate_transaction_from_edits(&source, edits, OffsetEncoding::Utf16),
            Err(TextEditError::Overlapping {
                edit_idx: 2,
                previous_edit_idx: 1
            })
        ));
        assert_eq!(source, "abcdef");
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
        let transaction =
            generate_transaction_from_edits(&source, edits, OffsetEncoding::Utf16).unwrap();
        assert!(transaction.apply(&mut source));
        assert_eq!(source, "XbcdYf");
    }

    #[test]
    fn equal_position_inserts_keep_server_order() {
        let mut source = Rope::from_str("abc");
        let edits = ["first", "second"].map(|text| lsp::TextEdit {
            range: lsp::Range::default(),
            new_text: text.into(),
        });
        let transaction =
            generate_transaction_from_edits(&source, edits.into(), OffsetEncoding::Utf8).unwrap();
        assert!(transaction.apply(&mut source));
        assert_eq!(source, "firstsecondabc");
    }

    #[test]
    fn positions_inside_encoded_characters_are_rejected() {
        let source = Rope::from_str("a😀z\r\n");
        for (encoding, character) in [(OffsetEncoding::Utf8, 2), (OffsetEncoding::Utf16, 2)] {
            let position = lsp::Position::new(0, character);
            let edits = vec![lsp::TextEdit {
                range: lsp::Range::new(position, position),
                new_text: "bad".into(),
            }];
            assert_eq!(
                generate_transaction_from_edits(&source, edits, encoding).unwrap_err(),
                TextEditError::InvalidPosition {
                    edit_idx: 0,
                    position
                }
            );
        }
    }

    #[test]
    fn encoded_ranges_and_line_end_clamping_remain_supported() {
        for (encoding, end) in [
            (OffsetEncoding::Utf8, 5),
            (OffsetEncoding::Utf16, 3),
            (OffsetEncoding::Utf32, 2),
        ] {
            let mut source = Rope::from_str("a😀z\r\n");
            let edits = vec![
                lsp::TextEdit {
                    range: lsp::Range::new(lsp::Position::new(0, 1), lsp::Position::new(0, end)),
                    new_text: "X".into(),
                },
                lsp::TextEdit {
                    range: lsp::Range::new(lsp::Position::new(0, 100), lsp::Position::new(0, 100)),
                    new_text: "!".into(),
                },
            ];
            let transaction = generate_transaction_from_edits(&source, edits, encoding).unwrap();
            assert!(transaction.apply(&mut source));
            assert_eq!(source, "aXz!\r\n");
        }
    }
}
