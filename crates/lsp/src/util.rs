//! Conversion between protocol values and editor text positions and edits.

use crate::{lsp, OffsetEncoding};
use editor_core::line_ending::get_line_ending;
use editor_core::{Range, Rope};

/// Converts [`lsp::Position`] to a position in the document.
///
/// Out-of-range lines resolve to EOF. Columns are capped before the line terminator.
pub fn lsp_pos_to_pos(
    doc: &Rope,
    pos: lsp::Position,
    offset_encoding: OffsetEncoding,
) -> Option<usize> {
    let pos_line = pos.line as usize;
    if pos_line > doc.len_lines(stdx::rope::LINE_TYPE) - 1 {
        // If it extends past the end, truncate it to the end. This is because the
        // way the LSP describes the range including the last newline is by
        // specifying a line number after what we would call the last line.
        log::warn!("LSP position {pos:?} out of range assuming EOF");
        return Some(doc.len_chars());
    }

    // We need to be careful here to fully comply with the LSP spec.
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
    // so we exclude the terminator before converting the column.
    //
    // FIXME: Mitos does not fully comply with the LSP spec for line terminators.
    // The LSP standard requires that line terminators are ['\n', '\r\n', '\r'].
    // Without the unicode-linebreak feature disabled, the `\r` terminator is not handled by mitos.
    // With the unicode-linebreak feature, mitos recognizes multiple extra line break chars
    // which means that positions will be decoded/encoded incorrectly in their presence

    let start = doc.line_to_byte_idx(pos_line, stdx::rope::LINE_TYPE);
    let end = doc.line_to_byte_idx(pos_line + 1, stdx::rope::LINE_TYPE);
    let line = doc.slice(start..end);
    let ending = get_line_ending(&line).map_or(0, |ending| ending.as_str().len());
    let line = line.slice(..line.len() - ending);
    // Short lines can use contiguous UTF-8 directly. Long lines keep rope metrics.
    let line = line.as_str().map_or(line, editor_core::RopeSlice::from);
    let column = pos.character as usize;
    let offset = match offset_encoding {
        OffsetEncoding::Utf8 => line.byte_to_char_idx(column.min(line.len())),
        OffsetEncoding::Utf16 => {
            line.byte_to_char_idx(line.utf16_to_byte_idx(column.min(line.len_utf16())))
        }
        OffsetEncoding::Utf32 => column.min(line.len_chars()),
    };
    Some(doc.byte_to_char_idx(start) + offset)
}

/// Converts position in the document to [`lsp::Position`].
///
/// Panics when `pos` is out of `doc` bounds or operation overflows.
pub fn pos_to_lsp_pos(doc: &Rope, pos: usize, offset_encoding: OffsetEncoding) -> lsp::Position {
    let byte_idx = doc.char_to_byte_idx(pos);
    let line = doc.byte_to_line_idx(byte_idx, stdx::rope::LINE_TYPE);
    let line_start = doc.line_to_byte_idx(line, stdx::rope::LINE_TYPE);
    let col = match offset_encoding {
        OffsetEncoding::Utf8 => byte_idx - line_start,
        OffsetEncoding::Utf16 => {
            doc.byte_to_utf16_idx(byte_idx) - doc.byte_to_utf16_idx(line_start)
        }
        OffsetEncoding::Utf32 => pos - doc.byte_to_char_idx(line_start),
    };
    lsp::Position::new(line as u32, col as u32)
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

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::Rope;

    #[test]
    fn unicode_protocol_positions_cross_chunks_and_clamp_before_crlf() {
        let lines = 512;
        let text = Rope::from_str(&format!("{}終", "α😀e\u{301}\r\n".repeat(lines)));
        for line in [0, lines - 1] {
            for (encoding, columns) in [
                (OffsetEncoding::Utf8, [0, 2, 6, 7, 9]),
                (OffsetEncoding::Utf16, [0, 1, 3, 4, 5]),
                (OffsetEncoding::Utf32, [0, 1, 2, 3, 4]),
            ] {
                for (offset, column) in columns.into_iter().enumerate() {
                    let expected = lsp::Position::new(line as u32, column);
                    let position = line * 6 + offset;
                    assert_eq!(pos_to_lsp_pos(&text, position, encoding), expected);
                    assert_eq!(lsp_pos_to_pos(&text, expected, encoding), Some(position));
                }
                assert_eq!(
                    lsp_pos_to_pos(&text, lsp::Position::new(line as u32, u32::MAX), encoding),
                    Some(line * 6 + 4)
                );
            }
            for (encoding, column) in [(OffsetEncoding::Utf8, 3), (OffsetEncoding::Utf16, 2)] {
                assert_eq!(
                    lsp_pos_to_pos(&text, lsp::Position::new(line as u32, column), encoding),
                    Some(line * 6 + 1)
                );
            }
        }
    }

    #[test]
    fn protocol_positions_on_long_lines_keep_rope_metrics() {
        let source = format!("{}\r\n", "α😀".repeat(20_000));
        let text = Rope::from_str(&source);
        for position in [0, 1, 8191, 20_001, 40_000] {
            let byte = source.char_indices().nth(position).unwrap().0;
            for (encoding, column) in [
                (OffsetEncoding::Utf8, byte),
                (OffsetEncoding::Utf16, source[..byte].encode_utf16().count()),
                (OffsetEncoding::Utf32, position),
            ] {
                let protocol = lsp::Position::new(0, column as u32);
                assert_eq!(pos_to_lsp_pos(&text, position, encoding), protocol);
                assert_eq!(lsp_pos_to_pos(&text, protocol, encoding), Some(position));
            }
        }
    }

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
}
