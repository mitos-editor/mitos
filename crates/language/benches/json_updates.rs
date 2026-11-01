//! Full edit/inverse syntax updates; query/grammar construction is outside timing.
use editor_core::{ChangeSet, Rope};
use language::{loader::default_lang_loader, Syntax};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};
fn main() {
    let loader = default_lang_loader(loader::syntax::Resources::default());
    let language = loader.language_for_name("json").unwrap();
    for count in [1_000, 10_000, 100_000] {
        for slash in [false, true] {
            let entry = if slash {
                "{\"name\":\"value/path\"},\n"
            } else {
                "{\"name\":\"value\"},\n"
            };
            let text = Rope::from_str(&format!("[{}{{}}]\n", entry.repeat(count)));
            let mut syntax = Syntax::new_with_timeout(
                text.slice(..),
                language,
                &loader,
                Duration::from_secs(10),
            )
            .unwrap();
            let pos = 1 + (count / 2) * entry.len();
            let changes = ChangeSet::from_change(&text, (pos, pos, Some(" ".into())));
            let mut after = text.clone();
            assert!(changes.apply(&mut after));
            let undo = changes.invert(&text);
            let mut run = || {
                syntax
                    .update(text.slice(..), after.slice(..), &changes, &loader)
                    .unwrap();
                syntax
                    .update(after.slice(..), text.slice(..), &undo, &loader)
                    .unwrap();
                black_box(&syntax);
            };
            for _ in 0..3 {
                run();
            }
            let mut samples: Vec<_> = (0..15)
                .map(|_| {
                    let t = Instant::now();
                    run();
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            samples.sort_by(f64::total_cmp);
            println!(
                "elements={count} bytes={} slash={slash} pair_ms={:.6}",
                text.len(),
                samples[7]
            );
        }
    }
}
