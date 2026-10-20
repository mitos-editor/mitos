//! Benchmarks for the word index.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use editor_core::{Rope, Transaction};
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

/// Just extracting words from a rope (i.e. the hot path).
fn words(c: &mut Criterion) {
    let mut group = c.benchmark_group("words");
    for (name, text) in CORPORA {
        let rope = Rope::from_str(text);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &rope, |b, rope| {
            b.iter(|| {
                let mut count = 0usize;
                for word in extract_words(black_box(rope.slice(..))) {
                    black_box(word);
                    count += 1;
                }
                count
            });
        });
    }
    group.finish();
}

/// End-to-end extraction plus hashing and inserting.
fn add_document(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_document");
    for (name, text) in CORPORA {
        let rope = Rope::from_str(text);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &rope, |b, rope| {
            b.iter(|| {
                let mut index = WordIndex::default();
                index.add_document(black_box(rope), || false);
                index
            });
        });
    }
    group.finish();
}

fn cancellation(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancelled_index");
    for (name, body) in [
        ("words", "some_word "),
        ("punctuation", ". "),
        ("oversized_token", "a"),
    ] {
        let text = Rope::from(body.repeat(1_000_000));
        let empty = Rope::new();
        let edit = Transaction::change(
            &empty,
            std::iter::once((0, 0, Some(text.to_string().into()))),
        );
        group.bench_function(name, |b| {
            b.iter(|| {
                WordIndex::default().update_document(
                    black_box(&empty),
                    black_box(&text),
                    black_box(edit.changes()),
                    || black_box(true),
                );
            })
        });
    }
    group.finish();
}

criterion_group!(benches, words, add_document, cancellation);
criterion_main!(benches);
