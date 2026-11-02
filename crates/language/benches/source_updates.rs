//! Ordinary source edits; parse/query setup is outside timing.
use editor_core::{ChangeSet, Rope};
use language::{loader::default_lang_loader, Syntax};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};
fn main() {
    let loader = default_lang_loader(loader::syntax::Resources::default());
    loader.set_scopes(
        [
            "comment",
            "function",
            "keyword",
            "string",
            "variable",
            "variable.parameter",
            "type",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    let language = loader.language_for_name("rust").unwrap();
    let source = std::env::var("MITOS_BENCH_FIXTURE")
        .ok()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .unwrap_or_else(|| include_str!("../../view/src/document.rs").to_owned());
    let text = Rope::from_str(&source);
    for symbol in ["cursor_shape", "set_selection"] {
        benchmark(&source, &text, symbol, language, &loader);
    }
}

fn benchmark(
    source: &str,
    text: &Rope,
    symbol: &str,
    language: language::Language,
    loader: &language::Loader,
) {
    let byte = source
        .find(&format!("fn {symbol}"))
        .expect("fixture must contain the benchmark function")
        + 3;
    let pos = source[..byte].chars().count();
    let changes = ChangeSet::from_change(text, (pos, pos, Some("x".into())));
    let mut after = text.clone();
    assert!(changes.apply(&mut after));
    let undo = changes.invert(text);
    let mut syntax =
        Syntax::new_with_timeout(text.slice(..), language, loader, Duration::from_secs(10))
            .unwrap();
    let mut run = || {
        syntax
            .update(text.slice(..), after.slice(..), &changes, loader)
            .unwrap();
        syntax
            .update(after.slice(..), text.slice(..), &undo, loader)
            .unwrap();
        black_box(&syntax);
    };
    for _ in 0..3 {
        run();
    }
    let mut samples = (0..21)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    println!(
        "Rust symbol={symbol} source={} bytes; edit+inverse p50={:.6}ms p95={:.6}ms; samples=21",
        source.len(),
        samples[10],
        samples[19]
    );
}
