//! Full snapshot search, including regex matching, UTF-8 checks and result collection.
use editor_core::Rope;
use search::Search;
use std::{hint::black_box, path::Path, time::Instant};

fn main() {
    for (name, unit, pattern, expected) in [
        (
            "short-ascii",
            "prefix needle suffix\n".to_owned(),
            "needle",
            4000,
        ),
        (
            "short-unicode",
            "é😀 prefix needle 界 suffix\n".to_owned(),
            "needle",
            4000,
        ),
        (
            "long-line",
            format!("{}needle\n", "abcdef ".repeat(1000)),
            "needle",
            4000,
        ),
        (
            "multiline",
            "é😀 first\nsecond\nthird 界\nunmatched\n".to_owned(),
            r"first\nsecond\nthird",
            4000,
        ),
        (
            "whole-document",
            "é😀 first\nsecond\nthird 界\n".to_owned(),
            r"(?s)\A.*\z",
            1,
        ),
    ] {
        let text = Rope::from_str(&unit.repeat(4000));
        let search = Search::new(pattern, false).unwrap();
        let run = || {
            search
                .search_document(Path::new("snapshot"), black_box(&text))
                .unwrap()
        };
        assert_eq!(run().len(), expected);
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
            "{name}: bytes={} matches={expected} median_ms={:.6}",
            unit.len() * 4000,
            samples[10]
        );
    }
}
