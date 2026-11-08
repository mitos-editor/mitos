//! Public normalization path used by paste, replacement and reflow.
use editor_core::line_ending::{normalize_line_endings, LineEnding};
use std::{borrow::Cow, hint::black_box, sync::LazyLock, time::Instant};

// Preserve the previous implementation for paired comparisons in one process.
fn regex_normalize(text: &str, ending: LineEnding) -> Cow<'_, str> {
    static REGEX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\r\n|\r|\n").unwrap());
    REGEX.replace_all(text, ending.as_str())
}

fn measure(mut run: impl FnMut(), batch: usize) -> f64 {
    let start = Instant::now();
    for _ in 0..batch {
        run();
    }
    start.elapsed().as_secs_f64() * 1_000_000.0 / batch as f64
}

fn main() {
    for (name, text) in [
        ("short", "first\r\nsecond\rthird\n".to_owned()),
        ("no-endings", "é😀abcdef ".repeat(100_000)),
        ("lf", "é😀 first line\nsecond line\n".repeat(20_000)),
        ("crlf", "é😀 first line\r\nsecond line\r\n".repeat(20_000)),
        (
            "mixed",
            "é😀 first\r\nsecond\rthird\nfourth\u{2028}".repeat(20_000),
        ),
        (
            "sparse",
            format!("{}\r\n", "abcdef ".repeat(1000)).repeat(100),
        ),
    ] {
        for ending in [LineEnding::LF, LineEnding::Crlf] {
            let mut reference = || {
                black_box(regex_normalize(black_box(&text), ending));
            };
            let mut current = || {
                black_box(normalize_line_endings(black_box(&text), ending));
            };
            assert_eq!(
                normalize_line_endings(&text, ending),
                regex_normalize(&text, ending)
            );
            for _ in 0..3 {
                reference();
                current();
            }
            let batch = if text.len() < 100 { 1000 } else { 1 };
            let mut before = Vec::new();
            let mut after = Vec::new();
            for sample in 0..31 {
                // Alternate order to reduce the effect of frequency and temperature changes.
                if sample % 2 == 0 {
                    before.push(measure(&mut reference, batch));
                    after.push(measure(&mut current, batch));
                } else {
                    after.push(measure(&mut current, batch));
                    before.push(measure(&mut reference, batch));
                }
            }
            before.sort_by(f64::total_cmp);
            after.sort_by(f64::total_cmp);
            println!(
                "{name} {ending:?}: bytes={} regex_us={:.3} memchr_us={:.3} speedup={:.2}x",
                text.len(),
                before[15],
                after[15],
                before[15] / after[15]
            );
        }
    }
}
