//! Full public replacement planning, including rope flattening and capture expansion.
use editor_core::Rope;
use search::Search;
use std::{hint::black_box, time::Instant};
fn main() {
    for unicode in [false, true] {
        let unit = if unicode {
            "é😀 word text\n"
        } else {
            "word other text\n"
        };
        let prefix = if unicode { 3 } else { 0 };
        for count in [1_000, 10_000, 100_000] {
            let text = Rope::from_str(&unit.repeat(count));
            let search = Search::new("(?P<word>word)", false).unwrap();
            for (name, start) in [
                ("first", prefix),
                ("last", (count - 1) * unit.chars().count() + prefix),
            ] {
                let run = || {
                    search
                        .replacement_changes(text.slice(..), "${word}_new", Some(start..start + 4))
                        .unwrap()
                };
                assert_eq!(run(), vec![(start, start + 4, Some("word_new".into()))]);
                for _ in 0..3 {
                    black_box(run());
                }
                let mut samples: Vec<_> = (0..21)
                    .map(|_| {
                        let t = Instant::now();
                        black_box(run());
                        t.elapsed().as_secs_f64() * 1000.0
                    })
                    .collect();
                samples.sort_by(f64::total_cmp);
                println!(
                    "unicode={unicode} matches={count} target={name} median_ms={:.6}",
                    samples[10]
                );
            }
        }
    }
}
