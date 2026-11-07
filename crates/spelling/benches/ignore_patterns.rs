//! Compare the public filter with its previous per-pattern implementation.
use language_config::SpellingConfig;
use spelling::SpellingFilter;
use std::{collections::HashSet, hint::black_box, time::Instant};

struct Previous {
    min_word_length: usize,
    words: HashSet<String>,
    ignore: Vec<regex::Regex>,
}
impl Previous {
    fn new(config: &SpellingConfig) -> Self {
        Self {
            min_word_length: config.min_word_length(),
            words: config
                .words
                .iter()
                .map(|word| word.to_lowercase())
                .collect(),
            ignore: config
                .ignore_regexes
                .iter()
                .map(|p| regex::Regex::new(p).unwrap())
                .collect(),
        }
    }
    fn ignores(&self, word: &str) -> bool {
        word.chars().count() < self.min_word_length
            || self.words.contains(&word.to_lowercase())
            || self.ignore.iter().any(|regex| regex.is_match(word))
    }
}
fn compare(label: &str, mut previous: impl FnMut(), mut current: impl FnMut()) {
    for _ in 0..3 {
        previous();
        current();
    }
    let mut before = Vec::new();
    let mut after = Vec::new();
    for sample in 0..21 {
        let mut a = || {
            let start = Instant::now();
            previous();
            before.push(start.elapsed().as_secs_f64() * 1000.0);
        };
        let mut b = || {
            let start = Instant::now();
            current();
            after.push(start.elapsed().as_secs_f64() * 1000.0);
        };
        if sample % 2 == 0 {
            a();
            b();
        } else {
            b();
            a();
        }
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    println!(
        "{label}: previous_ms={:.6} current_ms={:.6} speedup={:.2}x",
        before[10],
        after[10],
        before[10] / after[10]
    );
}
fn main() {
    let words: Vec<_> = [
        "ordinary",
        "SCREAMING_CASE",
        "v123",
        "snake_case",
        "camelCase",
        "éclair",
        "internal0",
        "regularword",
        "Mitos",
        "teh",
    ]
    .into_iter()
    .cycle()
    .take(10_000)
    .collect();
    for count in [0, 1, 2, 4, 16, 64] {
        let patterns = (0..count)
            .map(|i| {
                if i == 0 {
                    "^[A-Z0-9_]+$".to_owned()
                } else if i == count - 1 {
                    "^v[0-9]+$".to_owned()
                } else {
                    format!("^internal{i}[A-Z]+$")
                }
            })
            .collect();
        let config = SpellingConfig {
            words: vec!["Mitos".into()],
            ignore_regexes: patterns,
            min_word_length: Some(2),
            ..Default::default()
        };
        let previous = Previous::new(&config);
        let current = SpellingFilter::new(&config);
        for word in &words {
            assert_eq!(previous.ignores(word), current.ignores(word));
        }
        compare(
            &format!("filter patterns={count} words=10000"),
            || {
                black_box(
                    words
                        .iter()
                        .filter(|word| previous.ignores(black_box(word)))
                        .count(),
                );
            },
            || {
                black_box(
                    words
                        .iter()
                        .filter(|word| current.ignores(black_box(word)))
                        .count(),
                );
            },
        );
        compare(
            &format!("construct patterns={count}"),
            || {
                black_box(Previous::new(black_box(&config)));
            },
            || {
                black_box(SpellingFilter::new(black_box(&config)));
            },
        );
        compare(
            &format!("construct-and-filter patterns={count} words=10000"),
            || {
                let filter = Previous::new(&config);
                black_box(
                    words
                        .iter()
                        .filter(|word| filter.ignores(black_box(word)))
                        .count(),
                );
            },
            || {
                let filter = SpellingFilter::new(&config);
                black_box(
                    words
                        .iter()
                        .filter(|word| filter.ignores(black_box(word)))
                        .count(),
                );
            },
        );
    }
}
