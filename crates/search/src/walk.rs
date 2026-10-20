//! Shared traversal policy for search and workspace file discovery.

use ignore::{DirEntry, WalkBuilder};
use std::path::{Path, PathBuf};

/// Directory traversal policy, supplied independently of application settings.
#[derive(Debug, Clone)]
pub struct Options {
    /// Omit version-control metadata even when ignore files are disabled.
    pub exclude_vcs: bool,
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub follow_symlinks: bool,
    pub deduplicate_links: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub max_depth: Option<usize>,
    pub custom_ignore_files: Vec<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            exclude_vcs: true,
            hidden: true,
            parents: true,
            ignore: true,
            follow_symlinks: true,
            deduplicate_links: true,
            git_ignore: true,
            git_global: true,
            git_exclude: true,
            max_depth: None,
            custom_ignore_files: Vec::new(),
        }
    }
}

impl Options {
    /// Construct a walker with the shared policy. Callers may add sorting or type filters.
    pub fn walker(&self, root: &Path) -> WalkBuilder {
        let absolute_root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        let deduplicate_links = self.deduplicate_links;
        let exclude_vcs = self.exclude_vcs;
        let mut walker = WalkBuilder::new(root);
        walker
            .hidden(self.hidden)
            .parents(self.parents)
            .ignore(self.ignore)
            .follow_links(self.follow_symlinks)
            .git_ignore(self.git_ignore)
            .git_global(self.git_global)
            .git_exclude(self.git_exclude)
            .max_depth(self.max_depth)
            .filter_entry(move |entry| {
                should_include(entry, &absolute_root, deduplicate_links, exclude_vcs)
            });
        for filename in &self.custom_ignore_files {
            walker.add_custom_ignore_filename(filename);
        }
        walker
    }
}

/// Shared traversal policy for search and file/symbol pickers.
/// Excludes VCS internals and optionally symlinks into the search root.
pub fn include_entry(entry: &DirEntry, root: &Path, dedup_symlinks: bool) -> bool {
    should_include(entry, root, dedup_symlinks, true)
}

fn should_include(entry: &DirEntry, root: &Path, dedup_symlinks: bool, exclude_vcs: bool) -> bool {
    if exclude_vcs
        && matches!(
            entry.file_name().to_str(),
            Some(".git" | ".pijul" | ".jj" | ".hg" | ".svn")
        )
    {
        return false;
    }

    // We also ignore symlinks that point inside the current directory
    // if `dedup_links` is enabled.
    if dedup_symlinks && entry.path_is_symlink() {
        return entry
            .path()
            .canonicalize()
            .ok()
            .is_some_and(|path| !path.starts_with(root));
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_and_directory_walks_respect_metadata_and_depth_policies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        std::fs::create_dir_all(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/file"), "text").unwrap();
        let mut options = Options {
            hidden: false,
            parents: false,
            git_global: false,
            ..Options::default()
        };
        let collect = |options: &Options| {
            options
                .walker(dir.path())
                .build()
                .filter_map(Result::ok)
                .map(|entry| entry.path().strip_prefix(dir.path()).unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        let search = collect(&options);
        assert!(!search.contains(&PathBuf::from(".git")));
        assert!(search.contains(&PathBuf::from("nested/file")));
        options.exclude_vcs = false;
        options.max_depth = Some(1);
        let explorer = collect(&options);
        assert!(explorer.contains(&PathBuf::from(".git")));
        assert!(explorer.contains(&PathBuf::from("nested")));
        assert!(!explorer.contains(&PathBuf::from("nested/file")));
    }
}
