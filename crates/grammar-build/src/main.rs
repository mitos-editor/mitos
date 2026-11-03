use anyhow::Result;

// Fetch sources before cross-compilation in release CI without building the editor.
fn main() -> Result<()> {
    let sources = loader::config::grammar_sources(&workspace::ResolvedWorkspace::current());
    if let Some(diagnostic) = sources.skipped {
        eprintln!("{diagnostic}");
    }
    grammar_build::fetch_grammars(
        loader::grammar::get_grammar_configs(sources.value?)?,
        &loader::runtime_write_dir(),
        true,
    )
}
