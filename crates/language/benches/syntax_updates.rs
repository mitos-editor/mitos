//! Isolate full incremental syntax updates from a root-parser comparison.
use editor_core::{ChangeSet, Rope};
use language::{
    tree_sitter::{InputEdit, Parser, Point},
    Syntax,
};
use std::{
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

fn setting(name: &str, default: usize) -> anyhow::Result<usize> {
    match std::env::var(name) {
        Ok(value) => {
            let value = value.parse()?;
            anyhow::ensure!(value > 0, "{name} must be positive");
            Ok(value)
        }
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

fn report(language: &str, bytes: usize, edit: &str, stage: &str, times: &mut [u128]) {
    times.sort_unstable();
    let percentile = |p: usize| times[(times.len() * p).div_ceil(100).saturating_sub(1)];
    println!(
        "{language},{bytes},{edit},{stage},{},{},{},{}",
        times.len(),
        percentile(50),
        percentile(95),
        percentile(99)
    );
}

fn main() -> anyhow::Result<()> {
    let samples = setting("MITOS_BENCH_SAMPLES", 21)?;
    let rows = setting("MITOS_BENCH_ROWS", 1_000)?;
    let runtime = std::env::var_os("MITOS_BENCH_RUNTIME").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime"),
        PathBuf::from,
    );
    let loader =
        language::loader::default_lang_loader(loader::syntax::Resources::new(vec![runtime]));
    // Missing injected grammars would otherwise silently turn an injection
    // workload into a root-only parse.
    for name in [
        "rust-format-args",
        "rust-format-args-macro",
        "markdown.inline",
    ] {
        let language = loader
            .language_for_name(name)
            .ok_or_else(|| anyhow::anyhow!("Missing injected language {name}"))?;
        anyhow::ensure!(
            loader.grammar(language)?.is_some(),
            "Missing compiled injection grammar for {name}"
        );
    }
    println!("language,bytes,edit,stage,samples,p50_ns,p95_ns,p99_ns");
    for (name, line, prefix, suffix) in [
        ("json", "[1,2,3,4],\n", "[\n", "[]]\n"),
        (
            "rust",
            "fn item() { println!(\"value {}\", 42); }\n",
            "",
            "",
        ),
        (
            "markdown",
            "Prose **text**.\n\n```typescript\nconst value = 42;\n```\n\n",
            "",
            "",
        ),
        (
            "typescript",
            "function item() { return { value: 42 }; }\n",
            "",
            "",
        ),
    ] {
        let language = loader
            .language_for_name(name)
            .ok_or_else(|| anyhow::anyhow!("Missing language {name}"))?;
        let grammar = loader.grammar(language)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Missing compiled grammar {name}; prepare runtime grammars before benchmarking"
            )
        })?;
        let text = Rope::from_str(&format!("{prefix}{}{suffix}", line.repeat(rows)));
        // Cold construction is a distinct observation, never mixed into edit samples.
        let start = Instant::now();
        let syntax = Syntax::new(text.slice(..), language, &loader)
            .map_err(|error| anyhow::anyhow!("Initial syntax for {name}: {error:?}"))?;
        report(
            name,
            text.len(),
            "none",
            "first_construction",
            &mut [start.elapsed().as_nanos()],
        );
        let source = text.to_string();
        let expected: &[(&str, &str)] = match name {
            "rust" => &[
                ("value {}", "rust-format-args-macro"),
                ("value {}", "rust-format-args"),
            ],
            "markdown" => &[("text", "markdown.inline"), ("value", "typescript")],
            _ => &[],
        };
        for (marker, expected) in expected {
            let byte = source.find(marker).unwrap() as u32;
            anyhow::ensure!(
                syntax.layers_for_byte_range(byte, byte).any(|layer| {
                    loader
                        .language(syntax.layer(layer).language)
                        .config()
                        .language_id
                        == *expected
                }),
                "{name} fixture is missing its expected {expected} injection at {marker}"
            );
        }
        for (edit_name, inserted) in [("local", " "), ("quote", "\"")] {
            let pos = text.len_chars() / 2;
            let byte = text.char_to_byte_idx(pos) as u32;
            let changes = ChangeSet::from_change(&text, (pos, pos, Some(inserted.into())));
            let mut after = text.clone();
            anyhow::ensure!(changes.apply(&mut after), "Invalid fixture edit");
            let edit = InputEdit {
                start_byte: byte,
                old_end_byte: byte,
                new_end_byte: byte + inserted.len() as u32,
                start_point: Point::ZERO,
                old_end_point: Point::ZERO,
                new_end_point: Point::ZERO,
            };
            let mut full = Vec::new();
            let mut tree_edit = Vec::new();
            let mut root_parse = Vec::new();
            let mut parser = Parser::new();
            parser.set_grammar(grammar)?;
            for i in 0..samples + 3 {
                // Clone/setup/drop stay outside the timed regions. Alternate the
                // two paths to avoid systematically favouring a warmer parser.
                let mut live = syntax.clone();
                let mut tree = syntax.tree().clone();
                let mut measure_full = || -> anyhow::Result<_> {
                    let start = Instant::now();
                    live.update(text.slice(..), after.slice(..), &changes, &loader)
                        .map_err(|error| anyhow::anyhow!("Syntax update for {name}: {error:?}"))?;
                    Ok(start.elapsed().as_nanos())
                };
                let mut measure_root = || -> anyhow::Result<_> {
                    let start = Instant::now();
                    tree.edit(&edit);
                    let edit_ns = start.elapsed().as_nanos();
                    let start = Instant::now();
                    let parsed = parser
                        .parse_with_timeout(
                            after.slice(..),
                            Some(&tree),
                            Duration::from_millis(500),
                        )
                        .ok_or_else(|| anyhow::anyhow!("Root parser timed out for {name}"))?;
                    let parse_ns = start.elapsed().as_nanos();
                    black_box(&parsed);
                    Ok((edit_ns, parse_ns))
                };
                let (full_ns, (edit_ns, parse_ns)) = if i % 2 == 0 {
                    (measure_full()?, measure_root()?)
                } else {
                    let root = measure_root()?;
                    (measure_full()?, root)
                };
                black_box(live);
                if i >= 3 {
                    full.push(full_ns);
                    tree_edit.push(edit_ns);
                    root_parse.push(parse_ns);
                }
            }
            report(name, text.len(), edit_name, "full_update", &mut full);
            report(
                name,
                text.len(),
                edit_name,
                "root_tree_edit",
                &mut tree_edit,
            );
            report(name, text.len(), edit_name, "root_parse", &mut root_parse);
        }
    }
    Ok(())
}
