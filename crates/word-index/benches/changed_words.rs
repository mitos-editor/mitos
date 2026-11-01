//! Compares snapshot-window discovery and complete indexing updates separately.
//! Run unchanged fixtures on the base and proposed commits.
use editor_core::Rope;
use std::{hint::black_box, time::Instant};
use word_index::{changed_words, WordIndex};

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
            start.elapsed().as_secs_f64() * 1000.0 / iterations as f64
        })
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    samples[15]
}
fn measure(label: &str, before: &Rope, after: &Rope) {
    let window_ms = median(
        || {
            black_box(changed_words(
                black_box(before.slice(..)),
                black_box(after.slice(..)),
                || false,
            ));
        },
        10,
    );
    let mut index = WordIndex::default();
    index.add_document(before, || false);
    let update_pair_ms = median(
        || {
            for (a, b) in [(before, after), (after, before)] {
                if let Some((old, new)) =
                    changed_words(black_box(a.slice(..)), black_box(b.slice(..)), || false)
                {
                    index.replace_words(old, new, || false);
                }
            }
            black_box(&index);
        },
        1,
    );
    println!(
        "{label}: bytes={}, window_ms={window_ms:.6}, update_pair_ms={update_pair_ms:.6}",
        before.len()
    );
}
fn main() {
    for (name, unit, repeats) in [
        ("short", "identifier α😀 = value;\n", 4),
        ("ascii", "identifier = value;\n", 200_000),
        ("unicode", "identifier α😀 = value;\n", 180_000),
    ] {
        let before = Rope::from(unit.repeat(repeats));
        for (position, char_pos) in [
            ("start", 0),
            ("middle", before.len_chars() / 2),
            ("end", before.len_chars()),
        ] {
            let mut after = before.clone();
            after.insert(after.char_to_byte_idx(char_pos), "x");
            measure(&format!("{name}/{position}"), &before, &after);
        }
        let equal = Rope::from(before.to_string());
        measure(&format!("{name}/equal"), &before, &equal);
    }
}
