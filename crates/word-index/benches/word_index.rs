//! Benchmarks for the word index.

use std::{hint::black_box, time::Instant};

use editor_core::Rope;
use word_index::{words as extract_words, WordIndex};

/// See `texts/README`. These texts are used in unicode-segmentation's benchmarks.
const CORPORA: &[(&str, &str)] = &[
    ("arabic", include_str!("texts/arabic.txt")),
    ("english", include_str!("texts/english.txt")),
    ("hindi", include_str!("texts/hindi.txt")),
    ("japanese", include_str!("texts/japanese.txt")),
    ("korean", include_str!("texts/korean.txt")),
    ("mandarin", include_str!("texts/mandarin.txt")),
    ("russian", include_str!("texts/russian.txt")),
    ("source_code", include_str!("texts/source_code.txt")),
];

fn median(mut run: impl FnMut(), iterations: usize) -> f64 {
    for _ in 0..5 {
        run();
    }
    let mut samples = (0..31)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..iterations {
                run();
            }
            start.elapsed().as_secs_f64() * 1_000_000.0 / iterations as f64
        })
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    samples[15]
}

fn report(group: &str, corpus: &str, bytes: usize, median_us: f64) {
    let mib_per_s = bytes as f64 / (1024.0 * 1024.0) / (median_us / 1_000_000.0);
    println!("{group}/{corpus}: bytes={bytes}, median_us={median_us:.6}, mib_per_s={mib_per_s:.2}");
}

/// Just extracting words from a rope (i.e. the hot path).
fn words() {
    for (name, text) in CORPORA {
        let rope = Rope::from_str(text);
        let elapsed = median(
            || {
                let mut count = 0usize;
                for word in extract_words(black_box(rope.slice(..))) {
                    black_box(word);
                    count += 1;
                }
                black_box(count);
            },
            10,
        );
        report("words", name, text.len(), elapsed);
    }
}

/// End-to-end extraction plus hashing and inserting.
fn add_document() {
    for (name, text) in CORPORA {
        let rope = Rope::from_str(text);
        let elapsed = median(
            || {
                let mut index = WordIndex::default();
                index.add_document(black_box(&rope), || false);
                black_box(index);
            },
            10,
        );
        report("add_document", name, text.len(), elapsed);
    }
}

fn cancellation() {
    for (name, body) in [
        ("words", "some_word "),
        ("punctuation", ". "),
        ("oversized_token", "a"),
    ] {
        let text = Rope::from(body.repeat(1_000_000));
        let empty = Rope::new();
        let elapsed = median(
            || {
                WordIndex::default().replace_words(
                    black_box(empty.slice(..)),
                    black_box(text.slice(..)),
                    || black_box(true),
                );
            },
            1_000,
        );
        println!(
            "cancelled_index/{name}: bytes={}, median_us={elapsed:.6}",
            text.len()
        );
    }
}

fn main() {
    words();
    add_document();
    cancellation();
}
