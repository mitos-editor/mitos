//! Text workloads shared by the Ropey 1 baseline and the Ropey 2 migration.
//! See docs/ropey-performance.md for commands and measurement boundaries.

#[path = "support/allocations.rs"]
mod allocations;

use std::hint::black_box;

use criterion::Criterion;
use editor_core::{graphemes, Rope, Tendril, Transaction};
use lsp_client::{util, OffsetEncoding};
use stdx::rope::{Regex, RopeSliceExt};
use text_layout::{
    doc_formatter::{DocumentFormatter, TextFormat},
    text_annotations::TextAnnotations,
};

#[global_allocator]
static ALLOCATOR: allocations::Allocator = allocations::Allocator;

fn bench(c: &mut Criterion, name: &str, mut operation: impl FnMut()) {
    if std::env::var_os("MITOS_BENCH_ALLOCATIONS").is_some() {
        let (calls, bytes) = allocations::measure(&mut operation);
        println!("{name}: {calls} allocations, {bytes} requested bytes");
    } else {
        c.bench_function(name, |b| b.iter(&mut operation));
    }
}

fn workloads(c: &mut Criterion) {
    for (name, body) in [
        ("ascii", "let value = \"text\"; // a source line\n"),
        ("unicode", "let value = \"α界😀\"; // café 🇨🇿 👨‍👩‍👧‍👦\r\n"),
    ] {
        let source: String = (0..16_384).map(|i| format!("{i:05} {body}")).collect();
        let text = Rope::from_str(&source);
        let slice = text.slice(..);
        let middle = text.len_chars() / 2;
        let positions: Vec<_> = (1..=128).map(|i| text.len_chars() * i / 130).collect();

        bench(c, &format!("ropey/{name}/byte_boundaries"), || {
            for byte in source.len() / 2..source.len() / 2 + 128 {
                black_box(RopeSliceExt::floor_char_boundary(slice, black_box(byte)));
                black_box(RopeSliceExt::ceil_char_boundary(slice, black_box(byte)));
                black_box(RopeSliceExt::is_char_boundary(slice, black_box(byte)));
            }
        });

        for n in [1, 4096] {
            bench(c, &format!("ropey/{name}/graphemes_next/{n}"), || {
                black_box(graphemes::nth_next_grapheme_boundary(
                    slice,
                    black_box(middle),
                    n,
                ));
            });
            bench(c, &format!("ropey/{name}/graphemes_prev/{n}"), || {
                black_box(graphemes::nth_prev_grapheme_boundary(
                    slice,
                    black_box(middle),
                    n,
                ));
            });
        }

        for encoding in [
            OffsetEncoding::Utf8,
            OffsetEncoding::Utf16,
            OffsetEncoding::Utf32,
        ] {
            bench(c, &format!("ropey/{name}/lsp/{encoding:?}"), || {
                for &position in &positions {
                    let protocol = util::pos_to_lsp_pos(&text, black_box(position), encoding);
                    black_box(util::lsp_pos_to_pos(&text, protocol, encoding));
                }
            });
        }

        let format = TextFormat {
            soft_wrap: true,
            viewport_width: 100,
            ..TextFormat::default()
        };
        let annotations = TextAnnotations::default();
        bench(c, &format!("ropey/{name}/layout_start"), || {
            black_box(DocumentFormatter::new_at_prev_checkpoint(
                slice,
                &format,
                &annotations,
                black_box(middle),
            ));
        });
        bench(c, &format!("ropey/{name}/layout_viewport"), || {
            let formatter = DocumentFormatter::new_at_prev_checkpoint(
                slice,
                &format,
                &annotations,
                black_box(middle),
            );
            for grapheme in formatter.take(2048) {
                black_box(grapheme);
            }
        });

        for (label, cursors, insertion) in [
            ("type", &positions[..1], Tendril::from("x")),
            ("type_128_cursors", positions.as_slice(), Tendril::from("x")),
            (
                "paste",
                &positions[..1],
                Tendril::from("α😀 copied text\n".repeat(8192)),
            ),
        ] {
            bench(c, &format!("ropey/{name}/{label}"), || {
                let transaction = Transaction::change(
                    &text,
                    cursors
                        .iter()
                        .map(|&pos| (pos, pos, Some(insertion.clone()))),
                );
                let mut document = text.clone();
                black_box(transaction.apply(&mut document));
                black_box(document);
            });
        }

        let regex = Regex::new("value|α界😀").unwrap();
        bench(c, &format!("ropey/{name}/search"), || {
            black_box(regex.find_iter(black_box(slice).regex_input()).count());
        });

        let insertion = "inserted α😀\n".repeat(1024);
        for (label, at) in [
            ("head", 0),
            ("middle", source.len() / 2),
            ("tail", source.len()),
        ] {
            // Byte boundaries for fixture construction must be valid on both versions.
            let at = source
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(source.len()))
                .take_while(|&i| i <= at)
                .last()
                .unwrap_or(0);
            let after = Rope::from_str(&format!("{}{}{}", &source[..at], insertion, &source[at..]));
            bench(c, &format!("ropey/{name}/large_diff/{label}"), || {
                black_box(text_diff::compare_ropes(
                    black_box(&text),
                    black_box(&after),
                ));
            });
        }
    }
}

fn main() {
    let allocations = std::env::var_os("MITOS_BENCH_ALLOCATIONS").is_some();
    let mut criterion = if allocations {
        Criterion::default()
    } else {
        Criterion::default().configure_from_args()
    };
    workloads(&mut criterion);
    if !allocations {
        criterion.final_summary();
    }
}
