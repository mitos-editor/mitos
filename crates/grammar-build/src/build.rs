//! Compile prepared grammar sources without building the editor.
use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let target = match args.next().as_deref() {
        None => None,
        Some("--target") => Some(args.next().context("--target requires a target triple")?),
        Some("--help" | "-h") => {
            println!("Usage: grammar-build [--target <target-triple>]");
            return Ok(());
        }
        Some(argument) => bail!("unexpected argument: {argument}"),
    };
    if let Some(argument) = args.next() {
        bail!("unexpected argument: {argument}");
    }
    let sources = loader::config::grammar_sources(&workspace::ResolvedWorkspace::current());
    if let Some(diagnostic) = sources.skipped {
        eprintln!("{diagnostic}");
    }
    grammar_build::build_grammars(
        loader::grammar::get_grammar_configs(sources.value?)?,
        &loader::runtime_write_dir(),
        target,
        true,
    )
}
