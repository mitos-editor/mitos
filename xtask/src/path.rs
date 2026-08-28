use std::path::{Path, PathBuf};

pub fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn website_docs() -> PathBuf {
    project_root().join("website/src/content/docs/")
}

pub fn runtime() -> PathBuf {
    project_root().join("runtime")
}

pub fn ts_queries() -> PathBuf {
    runtime().join("queries")
}

pub fn themes() -> PathBuf {
    runtime().join("themes")
}

pub fn tests_indent() -> PathBuf {
    project_root().join("tests").join("indent")
}

pub fn tests_highlight() -> PathBuf {
    project_root()
        .join("tests")
        .join("query")
        .join("highlights")
}
