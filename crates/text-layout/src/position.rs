//! Coordinates after soft wrapping and display annotations.
//!
//! For positions within unwrapped document lines, use the text coordinate
//! helpers in `editor_core` instead.

use crate::DisplayPosition;
use crate::{
    doc_formatter::{DocumentFormatter, TextFormat},
    text_annotations::TextAnnotations,
};
use editor_core::RopeSlice;
use std::cmp::Ordering;

/// Returns the visual offset from the start of the first visual line
/// in the block that contains anchor.
/// Text is always wrapped at blocks, they usually correspond to
/// actual line breaks but for very long lines
/// softwrapping positions are estimated with an O(1) algorithm
/// to ensure consistent performance for large lines (currently unimplemented)
///
/// Usually you want to use `visual_offset_from_anchor` instead but this function
/// can be useful (and faster) if
/// * You already know the visual position of the block
/// * You only care about the horizontal offset (column) and not the vertical offset (row)
pub fn visual_offset_from_block(
    text: RopeSlice,
    anchor: usize,
    pos: usize,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
) -> (DisplayPosition, usize) {
    let mut last_pos = DisplayPosition::default();
    let formatter = DocumentFormatter::new_at_prev_checkpoint(text, text_fmt, annotations, anchor);
    let block_start = formatter.next_char_pos();

    for grapheme in formatter {
        last_pos = grapheme.visual_pos;
        if grapheme.char_idx + grapheme.source.covered_chars() > pos {
            return (grapheme.visual_pos, block_start);
        }
    }

    (last_pos, block_start)
}

/// Returns the height of the given text when softwrapping
pub fn softwrapped_dimensions(text: RopeSlice, text_fmt: &TextFormat) -> (usize, u16) {
    let last_pos =
        visual_offset_from_block(text, 0, usize::MAX, text_fmt, &TextAnnotations::default()).0;
    if last_pos.row == 0 {
        (1, last_pos.col as u16)
    } else {
        (last_pos.row + 1, text_fmt.viewport_width)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum VisualOffsetError {
    PosBeforeAnchorRow,
    PosAfterMaxRow,
}

/// Returns the visual offset from the start of the visual line
/// that contains anchor.
pub fn visual_offset_from_anchor(
    text: RopeSlice,
    anchor: usize,
    pos: usize,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
    max_rows: usize,
) -> Result<(DisplayPosition, usize), VisualOffsetError> {
    let formatter = DocumentFormatter::new_at_prev_checkpoint(text, text_fmt, annotations, anchor);
    let mut anchor_line = None;
    let mut found_pos = None;
    let mut last_pos = DisplayPosition::default();

    let block_start = formatter.next_char_pos();
    if pos < block_start {
        return Err(VisualOffsetError::PosBeforeAnchorRow);
    }

    for grapheme in formatter {
        last_pos = grapheme.visual_pos;

        if grapheme.char_idx + grapheme.source.covered_chars() > pos {
            if let Some(anchor_line) = anchor_line {
                last_pos.row -= anchor_line;
                return Ok((last_pos, block_start));
            } else if found_pos.is_none() {
                found_pos = Some(last_pos);
            }
        }
        if grapheme.char_idx + grapheme.source.covered_chars() > anchor && anchor_line.is_none() {
            if let Some(mut found_pos) = found_pos {
                return if found_pos.row == last_pos.row {
                    found_pos.row = 0;
                    Ok((found_pos, block_start))
                } else {
                    Err(VisualOffsetError::PosBeforeAnchorRow)
                };
            } else {
                anchor_line = Some(last_pos.row);
            }
        }

        if let Some(anchor_line) = anchor_line
            && grapheme.visual_pos.row >= anchor_line + max_rows
        {
            return Err(VisualOffsetError::PosAfterMaxRow);
        }
    }

    let anchor_line = anchor_line.unwrap_or(last_pos.row);
    last_pos.row -= anchor_line;

    Ok((last_pos, block_start))
}

/// Returns the char index on the visual line `row_offset` below the visual line of
/// the provided char index `anchor` that is closest to the supplied visual `column`.
///
/// If the targeted visual line is entirely covered by virtual text the last
/// char position before the virtual text and a virtual offset is returned instead.
///
/// If no (text) grapheme starts at exactly at the specified column the
/// start of the grapheme to the left is returned. If there is no grapheme
/// to the left (for example if the line starts with virtual text) then the positioning
/// of the next grapheme to the right is returned.
///
/// If the `line` coordinate is beyond the end of the file, the EOF
/// position will be returned.
///
/// If the `column` coordinate is past the end of the given line, the
/// line-end position (in this case, just before the line ending
/// character) will be returned.
///
/// # Returns
///
/// `(real_char_idx, virtual_lines)`
///
/// The nearest character idx "closest" (see above) to the specified visual offset
/// on the visual line is returned if the visual line contains any text:
/// If the visual line at the specified offset is a virtual line generated by a `LineAnnotation`
/// the previous char_index is returned, together with the remaining vertical offset (`virtual_lines`)
pub fn char_idx_at_visual_offset(
    text: RopeSlice,
    mut anchor: usize,
    mut row_offset: isize,
    column: usize,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
) -> (usize, usize) {
    let mut pos = anchor;
    // convert row relative to visual line containing anchor to row relative to a block containing anchor (anchor may change)
    loop {
        let (visual_pos_in_block, block_char_offset) =
            visual_offset_from_block(text, anchor, pos, text_fmt, annotations);
        row_offset += visual_pos_in_block.row as isize;
        anchor = block_char_offset;
        if row_offset >= 0 {
            break;
        }

        if block_char_offset == 0 {
            row_offset = 0;
            break;
        }
        // the row_offset is negative so we need to look at the previous block
        // set the anchor to the last char before the current block so that we can compute
        // the distance of this block from the start of the previous block
        pos = anchor;
        anchor -= 1;
    }

    char_idx_at_visual_block_offset(
        text,
        anchor,
        row_offset as usize,
        column,
        text_fmt,
        annotations,
    )
}

/// This function behaves the same as `char_idx_at_visual_offset`, except that
/// the vertical offset `row` is always computed relative to the block that contains `anchor`
/// instead of the visual line that contains `anchor`.
/// Usually `char_idx_at_visual_offset` is more useful but this function can be
/// used in some situations as an optimization when `visual_offset_from_block` was used
///
/// # Returns
///
/// `(real_char_idx, virtual_lines)`
///
/// See `char_idx_at_visual_offset` for details
pub fn char_idx_at_visual_block_offset(
    text: RopeSlice,
    anchor: usize,
    row: usize,
    column: usize,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
) -> (usize, usize) {
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_fmt, annotations, anchor);
    let mut last_char_idx = formatter.next_char_pos();
    let mut found_non_virtual_on_row = false;
    let mut last_row = 0;
    for grapheme in &mut formatter {
        match grapheme.visual_pos.row.cmp(&row) {
            Ordering::Equal => {
                if grapheme.visual_pos.col + grapheme.width() > column {
                    if !grapheme.is_virtual() {
                        return (grapheme.char_idx, 0);
                    } else if found_non_virtual_on_row {
                        return (last_char_idx, 0);
                    }
                } else if !grapheme.is_virtual() {
                    found_non_virtual_on_row = true;
                    last_char_idx = grapheme.char_idx;
                }
            }
            Ordering::Greater if found_non_virtual_on_row => return (last_char_idx, 0),
            Ordering::Greater => return (last_char_idx, row - last_row),
            Ordering::Less => {
                if !grapheme.is_virtual() {
                    last_row = grapheme.visual_pos.row;
                    last_char_idx = grapheme.char_idx;
                }
            }
        }
    }

    (formatter.next_char_pos(), 0)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::text_annotations::InlineAnnotation;
    use editor_core::Rope;
    use stdx::rope::RopeExt;

    #[test]
    fn test_visual_off_from_block() {
        let text = Rope::from("ḧëḷḷö\nẅöṛḷḋ");
        let slice = text.slice(..);
        let annot = TextAnnotations::default();
        let text_fmt = TextFormat::default();
        assert_eq!(
            visual_offset_from_block(slice, 0, 0, &text_fmt, &annot).0,
            DisplayPosition::new(0, 0)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 5, &text_fmt, &annot).0,
            DisplayPosition::new(0, 5)
        ); // position on \n
        assert_eq!(
            visual_offset_from_block(slice, 0, 6, &text_fmt, &annot).0,
            DisplayPosition::new(1, 0)
        ); // position on w
        assert_eq!(
            visual_offset_from_block(slice, 0, 7, &text_fmt, &annot).0,
            DisplayPosition::new(1, 1)
        ); // position on o
        assert_eq!(
            visual_offset_from_block(slice, 0, 10, &text_fmt, &annot).0,
            DisplayPosition::new(1, 4)
        ); // position on d

        // Test with wide characters.
        let text = Rope::from("今日はいい\n");
        let slice = text.slice(..);
        assert_eq!(
            visual_offset_from_block(slice, 0, 0, &text_fmt, &annot).0,
            DisplayPosition::new(0, 0)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 1, &text_fmt, &annot).0,
            DisplayPosition::new(0, 2)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 2, &text_fmt, &annot).0,
            DisplayPosition::new(0, 4)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 3, &text_fmt, &annot).0,
            DisplayPosition::new(0, 6)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 4, &text_fmt, &annot).0,
            DisplayPosition::new(0, 8)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 5, &text_fmt, &annot).0,
            DisplayPosition::new(0, 10)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 6, &text_fmt, &annot).0,
            DisplayPosition::new(1, 0)
        );

        // Test with grapheme clusters.
        let text = Rope::from("a̐éö̲\r\n");
        let slice = text.slice(..);
        assert_eq!(
            visual_offset_from_block(slice, 0, 0, &text_fmt, &annot).0,
            DisplayPosition::new(0, 0)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 2, &text_fmt, &annot).0,
            DisplayPosition::new(0, 1)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 4, &text_fmt, &annot).0,
            DisplayPosition::new(0, 2)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 7, &text_fmt, &annot).0,
            DisplayPosition::new(0, 3)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 9, &text_fmt, &annot).0,
            DisplayPosition::new(1, 0)
        );

        // Test with wide-character grapheme clusters.
        // TODO: account for cluster.
        let text = Rope::from("किमपि\n");
        let slice = text.slice(..);
        assert_eq!(
            visual_offset_from_block(slice, 0, 0, &text_fmt, &annot).0,
            DisplayPosition::new(0, 0)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 2, &text_fmt, &annot).0,
            DisplayPosition::new(0, 2)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 3, &text_fmt, &annot).0,
            DisplayPosition::new(0, 3)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 5, &text_fmt, &annot).0,
            DisplayPosition::new(0, 5)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 6, &text_fmt, &annot).0,
            DisplayPosition::new(1, 0)
        );

        // Test with tabs.
        let text = Rope::from("\tHello\n");
        let slice = text.slice(..);
        assert_eq!(
            visual_offset_from_block(slice, 0, 0, &text_fmt, &annot).0,
            DisplayPosition::new(0, 0)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 1, &text_fmt, &annot).0,
            DisplayPosition::new(0, 4)
        );
        assert_eq!(
            visual_offset_from_block(slice, 0, 2, &text_fmt, &annot).0,
            DisplayPosition::new(0, 5)
        );
    }
    #[test]
    fn test_char_idx_at_visual_row_offset_inline_annotation() {
        let text = Rope::from("foo\nbar");
        let slice = text.slice(..);
        let mut text_fmt = TextFormat::default();
        let annotations = [InlineAnnotation {
            text: "x".repeat(100).into(),
            char_idx: 3,
        }];
        text_fmt.soft_wrap = true;

        assert_eq!(
            char_idx_at_visual_offset(
                slice,
                0,
                1,
                0,
                &text_fmt,
                TextAnnotations::default().add_inline_annotations(&annotations, None)
            ),
            (2, 1)
        );
    }

    #[test]
    fn test_char_idx_at_visual_row_offset() {
        let text = Rope::from("ḧëḷḷö\nẅöṛḷḋ\nfoo");
        let slice = text.slice(..);
        let mut text_fmt = TextFormat::default();
        for i in 0isize..3isize {
            for j in -2isize..=2isize {
                if !(0..3).contains(&(i + j)) {
                    continue;
                }
                println!("{i} {j}");
                assert_eq!(
                    char_idx_at_visual_offset(
                        slice,
                        slice.line_to_char_idx(i as usize),
                        j,
                        3,
                        &text_fmt,
                        &TextAnnotations::default(),
                    )
                    .0,
                    slice.line_to_char_idx((i + j) as usize) + 3
                );
            }
        }

        text_fmt.soft_wrap = true;
        let mut softwrapped_text = "foo ".repeat(10);
        softwrapped_text.push('\n');
        let last_char = softwrapped_text.len() - 1;

        let text = Rope::from(softwrapped_text.repeat(3));
        let slice = text.slice(..);
        assert_eq!(
            char_idx_at_visual_offset(
                slice,
                last_char,
                0,
                0,
                &text_fmt,
                &TextAnnotations::default(),
            )
            .0,
            32
        );
        assert_eq!(
            char_idx_at_visual_offset(
                slice,
                last_char,
                -1,
                0,
                &text_fmt,
                &TextAnnotations::default(),
            )
            .0,
            16
        );
        assert_eq!(
            char_idx_at_visual_offset(
                slice,
                last_char,
                -2,
                0,
                &text_fmt,
                &TextAnnotations::default(),
            )
            .0,
            0
        );
        assert_eq!(
            char_idx_at_visual_offset(
                slice,
                softwrapped_text.len() + last_char,
                -2,
                0,
                &text_fmt,
                &TextAnnotations::default(),
            )
            .0,
            softwrapped_text.len()
        );

        assert_eq!(
            char_idx_at_visual_offset(
                slice,
                softwrapped_text.len() + last_char,
                -5,
                0,
                &text_fmt,
                &TextAnnotations::default(),
            )
            .0,
            0
        );
    }
}

#[cfg(test)]
mod replacement_tests {
    use super::*;
    use crate::{
        doc_formatter::DocumentFormatter,
        text_annotations::{InlineAnnotation, LineReplacement, Overlay},
    };
    use editor_core::Rope;

    #[test]
    fn replacements_preserve_source_rows_and_map_labels_to_their_anchor() {
        let text = Rope::from("préfix\n<<<<<<< HEAD\r\n界\tx\n");
        let start = 7;
        let end = start + "<<<<<<< HEAD".chars().count();
        let replacement = LineReplacement {
            range: start..end,
            text: "── Current: HEAD ──".into(),
        };
        let inline = [InlineAnnotation::new(start + 2, "hidden")];
        let overlays = [Overlay::new(start + 3, "X")];
        let mut annotations = TextAnnotations::default();
        annotations.add_line_replacement(&replacement, None);
        annotations.add_inline_annotations(&inline, None);
        annotations.add_overlay(&overlays, None);
        let format = TextFormat::default();
        for pos in start..end {
            assert_eq!(
                visual_offset_from_block(text.slice(..), 0, pos, &format, &annotations).0,
                DisplayPosition::new(1, 0)
            );
        }
        for col in 0..18 {
            assert_eq!(
                char_idx_at_visual_offset(text.slice(..), 0, 1, col, &format, &annotations),
                (start, 0)
            );
        }
        assert_eq!(
            visual_offset_from_anchor(text.slice(..), end, start, &format, &annotations, 10)
                .unwrap()
                .0,
            DisplayPosition::new(0, 0)
        );
        let after = end + 2;
        assert_eq!(
            visual_offset_from_block(text.slice(..), 0, after, &format, &annotations).0,
            DisplayPosition::new(2, 0)
        );
        let formatted =
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &format, &annotations, 0)
                .collect::<Vec<_>>();
        assert_eq!(
            formatted
                .iter()
                .filter(|g| g.line_idx == 1)
                .map(|g| g.doc_chars())
                .sum::<usize>(),
            end - start + 2
        );
        assert!(formatted
            .iter()
            .filter(|g| g.line_idx == 1)
            .all(|g| g.char_idx == start || g.char_idx == end));
        assert_eq!(text.to_string(), "préfix\n<<<<<<< HEAD\r\n界\tx\n");
    }

    #[test]
    fn wrapped_replacements_and_eof_keep_source_positions() {
        for suffix in ["", "\nnext\n"] {
            let text = Rope::from(format!("<<<<<<< HEAD{suffix}"));
            let replacement = LineReplacement {
                range: 0..12,
                text: "Current 界 HEAD".into(),
            };
            let mut annotations = TextAnnotations::default();
            annotations.add_line_replacement(&replacement, None);
            let format = TextFormat {
                soft_wrap: true,
                viewport_width: 5,
                max_wrap: 0,
                ..Default::default()
            };
            let mut formatter =
                DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &format, &annotations, 0);
            let graphemes: Vec<_> = formatter.by_ref().collect();
            assert_eq!(formatter.next_char_pos(), text.len_chars());
            for g in graphemes.iter().filter(|g| {
                matches!(
                    g.source,
                    crate::doc_formatter::GraphemeSource::Replacement { .. }
                )
            }) {
                assert_eq!(
                    char_idx_at_visual_offset(
                        text.slice(..),
                        0,
                        g.visual_pos.row as isize,
                        g.visual_pos.col,
                        &format,
                        &annotations
                    )
                    .0,
                    0
                );
            }
            for pos in 0..12 {
                assert_eq!(
                    visual_offset_from_block(text.slice(..), 0, pos, &format, &annotations).0,
                    DisplayPosition::new(0, 0)
                );
            }
        }
    }
}
