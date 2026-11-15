use crate::{
    doc_formatter::{DocumentFormatter, TextFormat},
    text_annotations::{Conceal, InlineAnnotation, TextAnnotations},
};
use editor_core::{highlight::Highlight, Rope};

#[test]
fn per_item_colors_preserve_layer_order_folds_and_restarts() {
    let text = Rope::from_str("a\t界😀é\nnext line\nlast");
    let before = [InlineAnnotation::new(2, "before")];
    let swatches = [
        InlineAnnotation::new(2, "red"),
        InlineAnnotation::new(2, "blue"),
        InlineAnnotation::new(7, "green"),
        InlineAnnotation::new(12, "yellow"),
    ];
    let colors = [
        Highlight::new(0xff0000),
        Highlight::new(0x0000ff),
        Highlight::new(0x00ff00),
        Highlight::new(0xffff00),
    ];
    let padding = [InlineAnnotation::new(2, " "), InlineAnnotation::new(7, " ")];
    let conceals = [Conceal { range: 1..6 }];
    for conceal in [false, true] {
        for wrap in [false, true] {
            let mut old = TextAnnotations::default();
            let mut new = TextAnnotations::default();
            let mut owned = TextAnnotations::default();
            for annotations in [&mut old, &mut new, &mut owned] {
                annotations.add_inline_annotations(&before, None);
                if conceal {
                    annotations.set_conceals(&conceals);
                }
            }
            for (item, color) in swatches.iter().zip(colors) {
                old.add_inline_annotations(std::slice::from_ref(item), Some(color));
            }
            new.add_inline_annotations_with_highlights(&swatches, &colors);
            owned.add_inline_annotations_with_optional_highlights(
                &swatches,
                colors.into_iter().map(Some).collect(),
            );
            old.add_inline_annotations(&padding, None);
            new.add_inline_annotations(&padding, None);
            owned.add_inline_annotations_with_optional_highlights(
                &padding,
                vec![None; padding.len()],
            );
            let format = TextFormat {
                soft_wrap: wrap,
                viewport_width: 17,
                ..TextFormat::default()
            };
            for start in [0, 12, 2, 18, 0] {
                let render = |annotations: &TextAnnotations| {
                    DocumentFormatter::new_at_prev_checkpoint(
                        text.slice(..),
                        &format,
                        annotations,
                        start,
                    )
                    .map(|grapheme| format!("{grapheme:?}"))
                    .collect::<Vec<_>>()
                };
                assert_eq!(render(&new), render(&old));
                assert_eq!(render(&owned), render(&old));
            }
        }
    }
}

#[test]
#[should_panic(expected = "assertion `left == right` failed")]
fn per_item_colors_require_equal_slice_lengths() {
    let annotations = [InlineAnnotation::new(0, "color")];
    TextAnnotations::default().add_inline_annotations_with_highlights(&annotations, &[]);
}

#[test]
fn inline_provenance_preserves_entry_identity_and_utf8_offsets_through_wrapping() {
    use unicode_segmentation::UnicodeSegmentation as _;
    let text = Rope::from_str("a tail\nnext");
    // Identical values at one anchor still identify separate entries.
    let inline = [
        InlineAnnotation::new(1, "界e\u{301} 😀 label"),
        InlineAnnotation::new(1, "界e\u{301} 😀 label"),
    ];
    for wrap in [false, true] {
        for width in [4, 8, 40] {
            let mut annotations = TextAnnotations::default();
            annotations.add_inline_annotations(&inline, None);
            let format = TextFormat {
                soft_wrap: wrap,
                viewport_width: width,
                max_wrap: 0,
                max_indent_retain: 0,
                ..Default::default()
            };
            let mut offsets = [Vec::new(), Vec::new()];
            for glyph in
                DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &format, &annotations, 0)
            {
                if let Some(source) = glyph.inline_annotation {
                    let index = inline
                        .iter()
                        .position(|entry| std::ptr::eq(entry, source.annotation))
                        .unwrap();
                    offsets[index].push(source.byte_offset);
                    assert_eq!(
                        source.annotation.text[source.byte_offset..]
                            .graphemes(true)
                            .next()
                            .unwrap(),
                        glyph.raw.to_string()
                    );
                } else {
                    assert!(!glyph.is_virtual());
                }
            }
            for (entry, offsets) in inline.iter().zip(offsets) {
                assert_eq!(
                    offsets,
                    entry
                        .text
                        .grapheme_indices(true)
                        .map(|(offset, _)| offset)
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}

#[test]
fn concealed_inline_entries_have_no_displayed_provenance() {
    let text = Rope::from_str("abcdef\nnext");
    let entries = [
        InlineAnnotation::new(1, "hidden"),
        InlineAnnotation::new(3, "visible"),
    ];
    let conceals = [Conceal { range: 1..3 }];
    let mut annotations = TextAnnotations::default();
    annotations
        .add_inline_annotations(&entries, None)
        .set_conceals(&conceals);
    let format = TextFormat::default();
    let sources =
        DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &format, &annotations, 0)
            .filter_map(|glyph| glyph.inline_annotation)
            .collect::<Vec<_>>();
    assert_eq!(sources.len(), "visible".len());
    assert!(sources
        .iter()
        .all(|source| std::ptr::eq(source.annotation, &entries[1])));
}
