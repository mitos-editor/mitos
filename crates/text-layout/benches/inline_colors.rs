use editor_core::{highlight::Highlight, Rope};
use std::{hint::black_box, time::Instant};
use text_layout::{
    doc_formatter::{DocumentFormatter, TextFormat},
    text_annotations::{InlineAnnotation, TextAnnotations},
};

fn main() {
    let text = Rope::from_str(&"some document text to format\n".repeat(10_000));
    let format = TextFormat::default();
    for offscreen in [true, false] {
        for count in [100, 1_000, 10_000] {
            let items: Vec<_> = (0..count)
                .map(|i| InlineAnnotation::new(if offscreen { 50_000 + i * 8 } else { i * 4 }, "■"))
                .collect();
            let colors: Vec<_> = (0..count)
                .map(|i| Highlight::new((i * 137) as u32))
                .collect();
            let mut checksums = Vec::new();
            for grouped in [false, true] {
                let mut annotations = TextAnnotations::default();
                if grouped {
                    annotations.add_inline_annotations_with_highlights(&items, &colors);
                } else {
                    for (item, color) in items.iter().zip(&colors) {
                        annotations
                            .add_inline_annotations(std::slice::from_ref(item), Some(*color));
                    }
                }
                let run = || {
                    DocumentFormatter::new_at_prev_checkpoint(
                        text.slice(..),
                        &format,
                        &annotations,
                        0,
                    )
                    .take(4_000)
                    .map(|g| g.char_idx)
                    .sum::<usize>()
                };
                checksums.push(run());
                for _ in 0..3 {
                    black_box(run());
                }
                let mut samples: Vec<_> = (0..21)
                    .map(|_| {
                        let start = Instant::now();
                        black_box(run());
                        start.elapsed().as_secs_f64() * 1000.0
                    })
                    .collect();
                samples.sort_by(f64::total_cmp);
                println!(
                    "offscreen={offscreen} colors={count} grouped={grouped} median_ms={:.6}",
                    samples[10]
                );
            }
            assert_eq!(checksums[0], checksums[1]);
        }
    }
}
