use anyhow::Result;

// Fetch sources before cross-compilation in release CI without building the editor.
fn main() -> Result<()> {
    grammar_build::fetch_grammars(
        loader::grammar::get_grammar_configs()?,
        &loader::runtime_dirs()[0],
        true,
    )
}
