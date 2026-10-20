use grammar_build::{build_grammars, fetch_grammars};

const STRICT: bool = true;

fn main() {
    if std::env::var("MITOS_DISABLE_AUTO_GRAMMAR_BUILD").is_err() {
        let runtime_dir = loader::runtime_write_dir();
        fetch_grammars(
            loader::grammar::get_grammar_configs().expect("Failed to load grammar configuration"),
            &runtime_dir,
            STRICT,
        )
        .expect("Failed to fetch tree-sitter grammars");
        build_grammars(
            loader::grammar::get_grammar_configs().expect("Failed to load grammar configuration"),
            &runtime_dir,
            Some(std::env::var("TARGET").unwrap()),
            STRICT,
        )
        .expect("Failed to compile tree-sitter grammars");
    }
}
