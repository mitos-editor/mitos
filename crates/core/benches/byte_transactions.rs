use editor_core::{Rope, Transaction};
use std::{hint::black_box, time::Instant};
fn main() {
    for (name, unit) in [
        ("ascii", "identifier = value;\n"),
        ("unicode", "identifier α😀 = value;\n"),
    ] {
        let before = Rope::from(unit.repeat(100_000));
        for edits in [1, 100, 10_000] {
            let step = before.len_chars() / (edits + 1);
            let tx = Transaction::change(
                &before,
                (1..=edits).map(|n| (n * step, n * step + 1, Some("界😀".into()))),
            );
            let mut samples = Vec::new();
            for _ in 0..31 {
                // Snapshot cloning is outside the timer; edits include normal copy-on-write costs.
                let mut text = before.clone();
                let start = Instant::now();
                assert!(black_box(&tx).apply(black_box(&mut text)));
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
                black_box(text);
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "{name}/{edits}_replacements: bytes={}, median_us={:.6}",
                before.len(),
                samples[15]
            );
        }
    }
}
