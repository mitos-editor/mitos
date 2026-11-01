//! Membership updates for a previously completed file walk.
use crate::{entry_file_type, Options};
use ignore::{types::Types, IncrementalIgnore};
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct FileIndex {
    root: PathBuf,
    canonical_root: PathBuf,
    paths: Vec<PathBuf>,
    matcher: IncrementalIgnore,
    exclude_vcs: bool,
    incremental: bool,
}

impl FileIndex {
    /// Scan using the same policy as subsequent updates, optionally streaming rows.
    /// Cancellation or a stopped sink discards the candidate. Traversal errors
    /// retain partial rows, but prevent their use as an incremental baseline.
    pub fn scan(
        root: &Path,
        options: &Options,
        types: Types,
        cancelled: impl Fn() -> bool,
        mut emit: impl FnMut(&Path) -> bool,
    ) -> Option<Self> {
        let mut builder = options.walker(root);
        builder.sort_by_file_name(|a, b| a.cmp(b)).types(types);
        let matcher = builder.build_matchers().into_iter().next()?;
        let canonical_root = root.canonicalize().ok();
        let mut index = Self {
            root: root.to_owned(),
            canonical_root: canonical_root.clone().unwrap_or_else(|| root.to_owned()),
            paths: Vec::new(),
            matcher,
            exclude_vcs: options.exclude_vcs,
            incremental: canonical_root.is_some()
                && !root
                    .symlink_metadata()
                    .is_ok_and(|metadata| metadata.file_type().is_symlink()),
        };
        for entry in builder.build() {
            if cancelled() {
                return None;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    index.incremental = false;
                    continue;
                }
            };
            index.incremental &= !entry.path_is_symlink() && entry.error().is_none();
            if entry_file_type(&entry).is_some_and(|kind| kind.is_file()) {
                if !emit(entry.path()) {
                    return None;
                }
                index.paths.push(entry.into_path());
            }
        }
        // A sorted depth-first walk is lexicographic order for leaf paths.
        debug_assert!(index.paths.is_sorted());
        (!cancelled()).then_some(index)
    }

    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.paths.iter()
    }

    /// Apply canonical filesystem paths to a complete, alias-free baseline.
    /// `None` requests a full scan. Unknown removals can be directory deletions;
    /// directory/symlink events and changed ignore rules must also be rescanned.
    /// The caller owns ignore-rule invalidation and lost-event/polling fallbacks.
    pub fn update(mut self, changed: &[PathBuf], cancelled: impl Fn() -> bool) -> Option<Self> {
        if !self.incremental || changed.is_empty() || changed.len() > 128 {
            return None;
        }
        if self.root.canonicalize().ok().as_ref() != Some(&self.canonical_root) {
            return None;
        }
        for canonical_path in changed {
            if cancelled() {
                return None;
            }
            let relative = canonical_path.strip_prefix(&self.canonical_root).ok()?;
            if relative.as_os_str().is_empty() {
                return None;
            }
            let path = self.root.join(relative);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    // IncrementalIgnore does not run the walker's custom entry predicate.
                    let vcs = self.exclude_vcs && relative.components().any(|part| {
                        matches!(part, Component::Normal(name) if crate::walk::is_vcs_name(name))
                    });
                    let (matched, errors) = self.matcher.matched_with_errors(relative, false);
                    if errors.is_some() {
                        return None;
                    }
                    match (self.paths.binary_search(&path), vcs || matched.is_ignore()) {
                        (Ok(index), true) => {
                            self.paths.remove(index);
                        }
                        (Err(index), false) => self.paths.insert(index, path),
                        _ => {}
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let index = self.paths.binary_search(&path).ok()?;
                    self.paths.remove(index);
                }
                _ => return None,
            }
        }
        (!cancelled()).then_some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn types() -> Types {
        let mut types = ignore::types::TypesBuilder::new();
        types.add("compressed", "*.zip").unwrap();
        types.negate("all");
        types.build().unwrap()
    }
    fn scan(root: &Path) -> FileIndex {
        let options = Options {
            parents: false,
            git_global: false,
            ..Options::default()
        };
        FileIndex::scan(root, &options, types(), || false, |_| true).unwrap()
    }
    #[test]
    fn updates_match_full_scans_and_keep_filtering_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join(".ignore"), "ignored.rs\n").unwrap();
        std::fs::write(root.join("nested/z.rs"), "").unwrap();
        let mut index = scan(&root);
        for path in [
            "new.rs",
            "nested/a.rs",
            "ignored.rs",
            "archive.zip",
            ".git/config",
            ".hidden",
        ] {
            std::fs::write(root.join(path), "").unwrap();
            index = index.update(&[root.join(path)], || false).unwrap();
            assert_eq!(
                index.paths().collect::<Vec<_>>(),
                scan(&root).paths().collect::<Vec<_>>()
            );
        }
        let deleted = root.join("nested/z.rs");
        std::fs::remove_file(&deleted).unwrap();
        index = index.update(&[deleted], || false).unwrap();
        assert_eq!(
            index.paths().collect::<Vec<_>>(),
            scan(&root).paths().collect::<Vec<_>>()
        );
    }
    #[test]
    fn directory_unknown_removal_empty_batch_and_cancellation_request_full_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(scan(&root).update(&[], || false).is_none());
        assert!(scan(&root)
            .update(&[root.join("missing")], || false)
            .is_none());
        std::fs::create_dir(root.join("directory")).unwrap();
        assert!(scan(&root)
            .update(&[root.join("directory")], || false)
            .is_none());
        std::fs::write(root.join("file"), "").unwrap();
        assert!(scan(&root).update(&[root.join("file")], || true).is_none());
        assert!(FileIndex::scan(&root, &Options::default(), types(), || true, |_| true).is_none());
        assert!(
            FileIndex::scan(&root, &Options::default(), types(), || false, |_| false).is_none()
        );
    }
    #[test]
    fn explicitly_selected_vcs_roots_keep_full_walk_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".git");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let index = scan(&root);
        assert_eq!(index.paths().count(), 0);
        std::fs::write(root.join("config"), "").unwrap();
        let index = index.update(&[root.join("config")], || false).unwrap();
        assert_eq!(
            index.paths().collect::<Vec<_>>(),
            scan(&root).paths().collect::<Vec<_>>()
        );
        assert_eq!(index.paths().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn aliases_disable_incremental_updates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("alias")).unwrap();
        std::fs::write(root.join("ordinary"), "").unwrap();
        assert!(scan(&root)
            .update(&[root.join("ordinary")], || false)
            .is_none());
    }
}
