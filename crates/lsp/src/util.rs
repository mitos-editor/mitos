//! Conversion between protocol values and editor text positions and edits.

use crate::{lsp, OffsetEncoding};
use editor_core::line_ending::{line_end_byte_index, line_end_char_index};
use editor_core::{Range, Rope};

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
}
