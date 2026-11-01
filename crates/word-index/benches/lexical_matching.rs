use editor_core::Rope;
use std::{hint::black_box, time::Instant};
use word_index::WordIndex;
fn main() {
    for unicode in [false, true] {
        for count in [1_000, 10_000, 100_000] {
            let prefixes = if unicode {
                ["parseÉntry", "préfixFile", "路径File", "filter項目"]
            } else {
                ["parseEntry", "prefixFile", "pathFile", "filterObject"]
            };
            let text = (0..count)
                .map(|i| {
                    format!(
                        "{}{}{:06} ",
                        prefixes[i % 4],
                        if i % 3 == 0 { "Buffer" } else { "Context" },
                        i
                    )
                })
                .collect::<String>();
            let mut index = WordIndex::default();
            index.add_document(&Rope::from_str(&text), || false);
            assert_eq!(index.word_counts().count(), count);
            let mut matcher = fuzzy::Matcher::default();
            for query in ["", "p", "pF", "xyz"] {
                let mut run = || index.matches(query, &mut matcher);
                let matched = run().len();
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
                println!("unicode={unicode} words={count} query={query} matched={matched} median_ms={:.6}",samples[10]);
            }
        }
    }
}
