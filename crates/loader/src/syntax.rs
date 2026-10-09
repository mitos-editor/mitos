//! Runtime grammar and query sources, independent of editor syntax compilation.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use tree_house::tree_sitter::Grammar;

/// An ordered set of runtime directories for one language setup.
///
/// Clones share the search paths, but reads remain lazy: files are resolved only
/// when requested. Each file uses the first existing path, including inherited
/// queries, so a higher-priority runtime can override just part of a language.
#[derive(Debug, Clone)]
pub struct Resources {
    directories: Arc<[PathBuf]>,
    owned_queries: Arc<BTreeMap<(String, String), String>>,
    query_aliases: Arc<BTreeMap<String, String>>,
}

impl Default for Resources {
    fn default() -> Self {
        Self::new(crate::runtime_dirs().to_vec())
    }
}

impl Resources {
    pub fn new(directories: Vec<PathBuf>) -> Self {
        Self {
            directories: directories.into(),
            owned_queries: Default::default(),
            query_aliases: Default::default(),
        }
    }

    fn runtime_file(&self, relative: &Path) -> PathBuf {
        crate::runtime_file_in(&self.directories, relative)
    }

    pub fn grammar(&self, name: &str) -> anyhow::Result<Option<Grammar>> {
        let mut relative = PathBuf::from("grammars").join(name);
        relative.set_extension(crate::grammar::DYLIB_EXTENSION);
        crate::grammar::load_language(name, &self.runtime_file(&relative))
    }

    /// Read one query file without inheritance, retaining I/O errors for health checks.
    pub fn query_file(&self, language: &str, filename: &str) -> std::io::Result<String> {
        if let Some(source) = self
            .owned_queries
            .get(&(language.to_owned(), filename.to_owned()))
        {
            return Ok(source.clone());
        }
        let language = self
            .query_aliases
            .get(language)
            .map(String::as_str)
            .unwrap_or(language);
        let relative = PathBuf::from("queries").join(language).join(filename);
        std::fs::read_to_string(self.runtime_file(&relative))
    }

    /// Owned query bytes and aliases never alter native grammar search roots.
    pub fn with_owned_queries(
        &self,
        queries: BTreeMap<(String, String), String>,
        aliases: BTreeMap<String, String>,
    ) -> Self {
        Self {
            directories: self.directories.clone(),
            owned_queries: Arc::new(queries),
            query_aliases: Arc::new(aliases),
        }
    }

    pub fn without_owned(&self) -> Self {
        Self {
            directories: self.directories.clone(),
            owned_queries: Default::default(),
            query_aliases: Default::default(),
        }
    }

    /// Resolve query inheritance using the same per-file runtime precedence.
    /// Missing or unreadable query files remain empty, matching editor behavior.
    pub fn query(&self, language: &str, filename: &str) -> String {
        tree_house::read_query(language, |language| {
            self.query_file(language, filename).unwrap_or_default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_queries_use_native_grammar_roots_and_restore_on_unload() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "rust", "highlights.scm", "; native highlights");
        write(root.path(), "rust", "indents.scm", "; native indents");
        let native = Resources::new(vec![root.path().into()]);
        let owned = native.with_owned_queries(
            [(
                ("fixture.rust".into(), "highlights.scm".into()),
                "; owned highlights".into(),
            )]
            .into(),
            [("fixture.rust".into(), "rust".into())].into(),
        );
        assert_eq!(
            owned.query("fixture.rust", "highlights.scm"),
            "; owned highlights"
        );
        assert_eq!(
            owned.query("fixture.rust", "indents.scm"),
            "; native indents"
        );
        assert_eq!(owned.query("rust", "highlights.scm"), "; native highlights");
        assert!(owned
            .without_owned()
            .query("fixture.rust", "highlights.scm")
            .is_empty());
        assert!(owned.grammar("never-installed").unwrap().is_none());
    }

    fn write(root: &Path, language: &str, filename: &str, text: &str) {
        let dir = root.join("queries").join(language);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(filename), text).unwrap();
    }

    #[test]
    fn inheritance_uses_per_file_precedence_for_every_parent() {
        let high = tempfile::tempdir().unwrap();
        let low = tempfile::tempdir().unwrap();
        write(
            low.path(),
            "child",
            "highlights.scm",
            "; inherits: parent\n; child\n",
        );
        write(low.path(), "parent", "highlights.scm", "; lower parent\n");
        write(
            high.path(),
            "parent",
            "highlights.scm",
            "; inherits: grand\n; upper parent\n",
        );
        write(low.path(), "grand", "highlights.scm", "; grand\n");
        write(low.path(), "child", "indents.scm", "; lower indents\n");
        let resources = Resources::new(vec![high.path().into(), low.path().into()]);
        let query = resources.query("child", "highlights.scm");
        assert!(query.contains("; upper parent"));
        assert!(!query.contains("; lower parent"));
        assert!(!query.contains("inherits:"));
        assert!(query.find("; grand").unwrap() < query.find("; upper parent").unwrap());
        assert!(query.find("; upper parent").unwrap() < query.find("; child").unwrap());
        assert_eq!(resources.query("child", "indents.scm"), "; lower indents\n");
        // Health checks inspect the local file instead of expanding inheritance.
        assert!(resources
            .query_file("child", "highlights.scm")
            .unwrap()
            .contains("inherits:"));
    }

    #[test]
    fn missing_and_unreadable_queries_remain_empty_without_falling_through() {
        let high = tempfile::tempdir().unwrap();
        let low = tempfile::tempdir().unwrap();
        write(
            low.path(),
            "test",
            "highlights.scm",
            "; valid lower query\n",
        );
        write(high.path(), "test", "highlights.scm", "");
        let resources = Resources::new(vec![high.path().into(), low.path().into()]);
        assert_eq!(resources.query("test", "highlights.scm"), "");
        std::fs::write(high.path().join("queries/test/highlights.scm"), [0xff]).unwrap();
        assert!(resources.query_file("test", "highlights.scm").is_err());
        assert_eq!(resources.query("test", "highlights.scm"), "");
        assert!(resources.query_file("missing", "highlights.scm").is_err());
        assert_eq!(resources.query("missing", "highlights.scm"), "");
    }

    #[test]
    fn independent_sources_read_lazily_without_changing_process_paths() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let a = Resources::new(vec![first.path().into()]);
        let b = Resources::new(vec![second.path().into()]);
        assert_eq!(a.query("test", "highlights.scm"), "");
        write(first.path(), "test", "highlights.scm", "; first\n");
        write(second.path(), "test", "highlights.scm", "; second\n");
        assert_eq!(a.query("test", "highlights.scm"), "; first\n");
        assert_eq!(b.query("test", "highlights.scm"), "; second\n");
        write(first.path(), "test", "highlights.scm", "; updated\n");
        assert_eq!(a.clone().query("test", "highlights.scm"), "; updated\n");
        assert_eq!(b.query("test", "highlights.scm"), "; second\n");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn missing_grammars_are_optional_but_invalid_libraries_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let resources = Resources::new(vec![dir.path().into()]);
        assert!(resources.grammar("resource_test").unwrap().is_none());
        let grammar_dir = dir.path().join("grammars");
        std::fs::create_dir(&grammar_dir).unwrap();
        std::fs::write(
            grammar_dir
                .join("resource_test")
                .with_extension(crate::grammar::DYLIB_EXTENSION),
            "invalid library",
        )
        .unwrap();
        assert!(resources.grammar("resource_test").is_err());
    }
}
