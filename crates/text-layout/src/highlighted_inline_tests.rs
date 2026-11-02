use crate::{
    doc_formatter::{DocumentFormatter, TextFormat},
    text_annotations::{Conceal, InlineAnnotation, LineReplacement, TextAnnotations},
};
use editor_core::{highlight::Highlight, Rope};

#[test]
fn per_item_colors_preserve_layer_order_folds_replacements_and_restarts() {
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
    let replacement = LineReplacement {
        range: 7..10,
        text: "replacement".into(),
    };
    for conceal in [false, true] {
        for replace in [false, true] {
            for wrap in [false, true] {
                let mut old = TextAnnotations::default();
                let mut new = TextAnnotations::default();
                for annotations in [&mut old, &mut new] {
                    annotations.add_inline_annotations(&before, None);
                    if conceal {
                        annotations.set_conceals(&conceals);
                    }
                    if replace {
                        annotations.add_line_replacement(&replacement, None);
                    }
                }
                for (item, color) in swatches.iter().zip(colors) {
                    old.add_inline_annotations(std::slice::from_ref(item), Some(color));
                }
                new.add_inline_annotations_with_highlights(&swatches, &colors);
                old.add_inline_annotations(&padding, None);
                new.add_inline_annotations(&padding, None);
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
                }
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
