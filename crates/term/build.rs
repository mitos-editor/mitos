use grammar_build::{build_grammars, fetch_grammars};

const STRICT: bool = true;

fn main() {
    if std::env::var("MITOS_DISABLE_AUTO_GRAMMAR_BUILD").is_err() {
        let runtime_dir = loader::runtime_write_dir();
        let sources = loader::config::grammar_sources(&workspace::ResolvedWorkspace::current());
        if let Some(diagnostic) = sources.skipped {
            eprintln!("{diagnostic}");
        }
        let grammars = loader::grammar::get_grammar_configs(
            sources.value.expect("Failed to load grammar configuration"),
        )
        .expect("Failed to load grammar configuration");
        fetch_grammars(grammars.clone(), &runtime_dir, STRICT)
            .expect("Failed to fetch tree-sitter grammars");
        build_grammars(
            grammars,
            &runtime_dir,
            Some(std::env::var("TARGET").unwrap()),
            STRICT,
        )
        .expect("Failed to compile tree-sitter grammars");
    }
}
