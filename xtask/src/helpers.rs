use std::path::{Path, PathBuf};

use crate::path;
use language::queries::QueryFeature;

/// Get the list of languages that support a particular tree-sitter
/// based feature.
pub fn ts_lang_support(feat: QueryFeature) -> Vec<String> {
    let queries_dir = path::ts_queries();

    find_files(&queries_dir, feat.runtime_filename())
        .iter()
        .map(|f| {
            // .../mitos/runtime/queries/python/highlights.scm
            let tail = f.strip_prefix(&queries_dir).unwrap(); // python/highlights.scm
            let lang = tail.components().next().unwrap(); // python
            lang.as_os_str().to_string_lossy().to_string()
        })
        .collect()
}

// naive implementation, but suffices for our needs
pub fn find_files(dir: &Path, filename: &str) -> Vec<PathBuf> {
    fn collect(dir: &Path, filename: &str, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, filename, files);
            } else if path.file_name().is_some_and(|name| name == filename) {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(dir, filename, &mut files);
    files
}
