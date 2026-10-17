//! Conversion between protocol values and editor text positions and edits.

use crate::{lsp, OffsetEncoding};
use editor_core::line_ending::{line_end_byte_index, line_end_char_index};
use editor_core::{chars, RopeSlice};
use editor_core::{diagnostic::NumberOrString, Range, Rope, Selection, Tendril, Transaction};
use snippets::{RenderedSnippet, Snippet, SnippetRenderCtx};

/// Converts a diagnostic in the document to [`lsp::Diagnostic`].
///
/// Panics when [`pos_to_lsp_pos`] would for an invalid range on the diagnostic.
pub fn diagnostic_to_lsp_diagnostic(
    doc: &Rope,
    diag: &editor_core::diagnostic::Diagnostic,
    offset_encoding: OffsetEncoding,
) -> lsp::Diagnostic {
    use editor_core::diagnostic::Severity::*;

    let range = Range::new(diag.range.start, diag.range.end);
    let severity = diag.severity.map(|s| match s {
        Hint => lsp::DiagnosticSeverity::HINT,
        Info => lsp::DiagnosticSeverity::INFORMATION,
        Warning => lsp::DiagnosticSeverity::WARNING,
        Error => lsp::DiagnosticSeverity::ERROR,
    });

    let code = match diag.code.clone() {
        Some(x) => match x {
            NumberOrString::Number(x) => Some(lsp::NumberOrString::Number(x)),
            NumberOrString::String(x) => Some(lsp::NumberOrString::String(x.into_string())),
        },
        None => None,
    };

    let new_tags: Vec<_> = diag
        .tags
        .iter()
        .map(|tag| match tag {
            editor_core::diagnostic::DiagnosticTag::Unnecessary => lsp::DiagnosticTag::UNNECESSARY,
            editor_core::diagnostic::DiagnosticTag::Deprecated => lsp::DiagnosticTag::DEPRECATED,
        })
        .collect();

    let tags = if !new_tags.is_empty() {
        Some(new_tags)
    } else {
        None
    };

    lsp::Diagnostic {
        range: range_to_lsp_range(doc, range, offset_encoding),
        severity,
        code,
        source: diag.source.clone().map(|source| source.into_string()),
        message: diag.message.clone().into_string(),
        related_information: None,
        tags,
        data: diag.data.clone(),
        ..Default::default()
    }
}

/// Converts [`lsp::Position`] to a position in the document.
///
/// Returns `None` if position.line is out of bounds or an overflow occurs
pub fn lsp_pos_to_pos(
    doc: &Rope,
    pos: lsp::Position,
    offset_encoding: OffsetEncoding,
) -> Option<usize> {
    let pos_line = pos.line as usize;
    if pos_line > doc.len_lines() - 1 {
        // If it extends past the end, truncate it to the end. This is because the
        // way the LSP describes the range including the last newline is by
        // specifying a line number after what we would call the last line.
        log::warn!("LSP position {pos:?} out of range assuming EOF");
        return Some(doc.len_chars());
    }

    // We need to be careful here to fully comply ith the LSP spec.
    // Two relevant quotes from the spec:
    //
    // https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#position
    // > If the character value is greater than the line length it defaults back
    // >  to the line length.
    //
    // https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocuments
    // > To ensure that both client and server split the string into the same
    // > line representation the protocol specifies the following end-of-line sequences:
    // > ‘\n’, ‘\r\n’ and ‘\r’. Positions are line end character agnostic.
    // > So you can not specify a position that denotes \r|\n or \n| where | represents the character offset.
    //
    // This means that while the line must be in bounds the `character`
    // must be capped to the end of the line.
    // Note that the end of the line here is **before** the line terminator
    // so we must use `line_end_char_index` instead of `doc.line_to_char(pos_line + 1)`
    //
    // FIXME: Mitos does not fully comply with the LSP spec for line terminators.
    // The LSP standard requires that line terminators are ['\n', '\r\n', '\r'].
    // Without the unicode-linebreak feature disabled, the `\r` terminator is not handled by mitos.
    // With the unicode-linebreak feature, mitos recognizes multiple extra line break chars
    // which means that positions will be decoded/encoded incorrectly in their presence

    let line = match offset_encoding {
        OffsetEncoding::Utf8 => {
            let line_start = doc.line_to_byte(pos_line);
            let line_end = line_end_byte_index(&doc.slice(..), pos_line);
            line_start..line_end
        }
        OffsetEncoding::Utf16 => {
            // TODO directly translate line index to char-idx
            // ropey can do this just as easily as utf-8 byte translation
            // but the functions are just missing.
            // Translate to char first and then utf-16 as a workaround
            let line_start = doc.line_to_char(pos_line);
            let line_end = line_end_char_index(&doc.slice(..), pos_line);
            doc.char_to_utf16_cu(line_start)..doc.char_to_utf16_cu(line_end)
        }
        OffsetEncoding::Utf32 => {
            let line_start = doc.line_to_char(pos_line);
            let line_end = line_end_char_index(&doc.slice(..), pos_line);
            line_start..line_end
        }
    };

    // The LSP spec demands that the offset is capped to the end of the line
    let pos = line
        .start
        .checked_add(pos.character as usize)
        .unwrap_or(line.end)
        .min(line.end);

    match offset_encoding {
        OffsetEncoding::Utf8 => doc.try_byte_to_char(pos).ok(),
        OffsetEncoding::Utf16 => doc.try_utf16_cu_to_char(pos).ok(),
        OffsetEncoding::Utf32 => Some(pos),
    }
}

/// Converts position in the document to [`lsp::Position`].
///
/// Panics when `pos` is out of `doc` bounds or operation overflows.
pub fn pos_to_lsp_pos(doc: &Rope, pos: usize, offset_encoding: OffsetEncoding) -> lsp::Position {
    match offset_encoding {
        OffsetEncoding::Utf8 => {
            let line = doc.char_to_line(pos);
            let line_start = doc.line_to_byte(line);
            let col = doc.char_to_byte(pos) - line_start;

            lsp::Position::new(line as u32, col as u32)
        }
        OffsetEncoding::Utf16 => {
            let line = doc.char_to_line(pos);
            let line_start = doc.char_to_utf16_cu(doc.line_to_char(line));
            let col = doc.char_to_utf16_cu(pos) - line_start;

            lsp::Position::new(line as u32, col as u32)
        }
        OffsetEncoding::Utf32 => {
            let line = doc.char_to_line(pos);
            let line_start = doc.line_to_char(line);
            let col = pos - line_start;

            lsp::Position::new(line as u32, col as u32)
        }
    }
}

/// Converts a range in the document to [`lsp::Range`].
pub fn range_to_lsp_range(doc: &Rope, range: Range, offset_encoding: OffsetEncoding) -> lsp::Range {
    let start = pos_to_lsp_pos(doc, range.from(), offset_encoding);
    let end = pos_to_lsp_pos(doc, range.to(), offset_encoding);

    lsp::Range::new(start, end)
}

pub fn lsp_range_to_range(
    doc: &Rope,
    mut range: lsp::Range,
    offset_encoding: OffsetEncoding,
) -> Option<Range> {
    // This is sort of an edgecase. It's not clear from the spec how to deal with
    // ranges where end < start. They don't make much sense but vscode simply caps start to end
    // and because it's not specified quite a few LS rely on this as a result (for example the TS server)
    if range.start > range.end {
        log::error!(
            "Invalid LSP range start {:?} > end {:?}, using an empty range at the end instead",
            range.start,
            range.end
        );
        range.start = range.end;
    }
    let start = lsp_pos_to_pos(doc, range.start, offset_encoding)?;
    let end = lsp_pos_to_pos(doc, range.end, offset_encoding)?;

    Some(Range::new(start, end))
}

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
            return editor_core::diff::compare_ropes(doc, &new_text);
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
    use editor_core::Rope;

    #[test]
    fn converts_lsp_pos_to_pos() {
        macro_rules! test_case {
            ($doc:expr, ($x:expr, $y:expr) => $want:expr) => {
                let doc = Rope::from($doc);
                let pos = lsp::Position::new($x, $y);
                assert_eq!($want, lsp_pos_to_pos(&doc, pos, OffsetEncoding::Utf16));
                assert_eq!($want, lsp_pos_to_pos(&doc, pos, OffsetEncoding::Utf8))
            };
        }

        test_case!("", (0, 0) => Some(0));
        test_case!("", (0, 1) => Some(0));
        test_case!("", (1, 0) => Some(0));
        test_case!("\n\n", (0, 0) => Some(0));
        test_case!("\n\n", (1, 0) => Some(1));
        test_case!("\n\n", (1, 1) => Some(1));
        test_case!("\n\n", (2, 0) => Some(2));
        test_case!("\n\n", (3, 0) => Some(2));
        test_case!("test\n\n\n\ncase", (4, 3) => Some(11));
        test_case!("test\n\n\n\ncase", (4, 4) => Some(12));
        test_case!("test\n\n\n\ncase", (4, 5) => Some(12));
        test_case!("", (u32::MAX, u32::MAX) => Some(0));
    }

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
