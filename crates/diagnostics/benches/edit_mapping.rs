use diagnostics::{Diagnostic, DiagnosticProvider, DiagnosticStore, Range, Severity};
use editor_core::{ChangeSet, Rope};
use std::{hint::black_box, time::Instant};
fn main() {
    for count in [1_000, 10_000, 100_000] {
        let text = Rope::from_str(&"word message\n".repeat(count + 1));
        let mut store = DiagnosticStore::default();
        store.replace(
            (0..count).map(|i| Diagnostic {
                range: Range {
                    start: i * 13,
                    end: i * 13 + 4,
                },
                starts_at_word: true,
                ends_at_word: true,
                zero_width: false,
                line: i,
                message: "diagnostic".into(),
                severity: Some(Severity::Warning),
                code: None,
                provider: DiagnosticProvider::Spelling,
                tags: vec![],
                source: None,
                data: None,
            }),
            &[],
            None,
        );
        for (name, pos) in [
            ("beginning", 0),
            ("middle", text.len_chars() / 2),
            ("end", text.len_chars()),
        ] {
            let changes = ChangeSet::from_change(&text, (pos, pos, Some("\n".into())));
            let mut after = text.clone();
            assert!(changes.apply(&mut after));
            let inverse = changes.invert(&text);
            let mut run = || {
                store.apply_changes(&changes, &after);
                store.apply_changes(&inverse, &text);
                black_box(store.as_slice());
            };
            for _ in 0..3 {
                run();
            }
            let mut samples: Vec<_> = (0..21)
                .map(|_| {
                    let start = Instant::now();
                    run();
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            samples.sort_by(f64::total_cmp);
            println!(
                "diagnostics={count} position={name} pair_ms={:.6}",
                samples[10]
            );
        }
    }
}
