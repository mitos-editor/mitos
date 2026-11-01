//! Serialization only; excludes protocol framing, logging and I/O.
use lsp_client::jsonrpc::{Notification, Version};
use std::{hint::black_box, time::Instant};
fn median(mut run: impl FnMut() -> usize, iterations: usize) -> f64 {
    for _ in 0..8 {
        black_box(run());
    }
    let mut samples = (0..31)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(run());
            }
            start.elapsed().as_secs_f64() * 1_000_000.0 / iterations as f64
        })
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    samples[15]
}
fn main() {
    for (name, text, iterations) in [
        ("small_unicode", "let α = \"😀\";\n".into(), 1_000),
        (
            "large_ascii",
            "fn example() { let value = 42; }\n".repeat(32_000),
            5,
        ),
        (
            "large_unicode",
            "fn αβγ() { let value = \"界😀\"; }\n".repeat(24_000),
            5,
        ),
    ] {
        let value = Notification { jsonrpc: Some(Version::V2), method: "textDocument/didOpen".into(), params: serde_json::json!({"textDocument":{"uri":"file:///project/main.rs","version":1,"text":text}}).into() };
        let baseline = serde_json::to_string(&value).unwrap();
        let serde_us = median(
            || serde_json::to_string(black_box(&value)).unwrap().len(),
            iterations,
        );
        let mut buffer = Vec::with_capacity(baseline.len());
        let sonic_reuse_us = median(
            || {
                buffer.clear();
                sonic_rs::to_writer(&mut buffer, black_box(&value)).unwrap();
                black_box(&buffer).len()
            },
            iterations,
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&buffer).unwrap(),
            serde_json::from_str::<serde_json::Value>(&baseline).unwrap()
        );
        println!(
            "{name}: bytes={}, serde_new_us={serde_us:.6}, sonic_reuse_us={sonic_reuse_us:.6}",
            baseline.len()
        );
    }
}
