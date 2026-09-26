//! Ignore rules and native-watch coverage filtering.
use super::Config;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::{
    borrow::Borrow,
    path::{Path, PathBuf},
    slice,
    sync::Arc,
};

fn build_ignore(paths: impl IntoIterator<Item = PathBuf> + Clone, dir: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(dir);
    for path in paths.clone() {
        if let Some(err) = builder.add(&path)
            && !err.is_io()
        {
            log::error!("failed to read ignorefile at {path:?}: {err}");
        }
    }
    match builder.build() {
        Ok(ignore) => (!ignore.is_empty()).then_some(ignore),
        Err(err) => {
            if !err.is_io() {
                log::error!(
                    "failed to read ignorefile at {:?}: {err}",
                    paths.into_iter().collect::<Vec<_>>()
                );
            }
            None
        }
    }
}

struct IgnoreFiles {
    root: PathBuf,
    ignores: Vec<Arc<Gitignore>>,
}

impl IgnoreFiles {
    fn new(
        workspace_ignore: Option<Arc<Gitignore>>,
        config: &Config,
        root: &Path,
        globals: &[Arc<Gitignore>],
    ) -> Self {
        let mut ignores = Vec::with_capacity(8);
        // .mitos/ignore
        if let Some(workspace_ignore) = workspace_ignore {
            ignores.push(workspace_ignore);
        }
        for ancestor in root.ancestors() {
            let ignore = if config.ignore {
                if config.git_ignore {
                    // the second path takes priority
                    build_ignore(
                        [ancestor.join(".gitignore"), ancestor.join(".ignore")],
                        ancestor,
                    )
                } else {
                    build_ignore([ancestor.join(".ignore")], ancestor)
                }
            } else if config.git_ignore {
                build_ignore([ancestor.join(".gitignore")], ancestor)
            } else {
                None
            };
            if let Some(ignore) = ignore {
                ignores.push(Arc::new(ignore));
            }
        }
        ignores.extend(globals.iter().cloned());
        Self {
            root: root.into(),
            ignores,
        }
    }

    fn shared_ignores(
        workspace: &Path,
        config: &Config,
    ) -> (Vec<Arc<Gitignore>>, Option<Arc<Gitignore>>) {
        let mut ignores = Vec::new();
        let workspace_ignore = build_ignore(
            [
                loader::config_dir().join("ignore"),
                workspace.join(".mitos/ignore"),
            ],
            workspace,
        )
        .map(Arc::new);
        if config.git_global {
            let (gitignore_global, err) = Gitignore::global();
            if let Some(err) = err
                && !err.is_io()
            {
                log::error!("failed to read global ignore file: {err}");
            }
            if !gitignore_global.is_empty() {
                ignores.push(Arc::new(gitignore_global));
            }
        }
        (ignores, workspace_ignore)
    }

    fn filesentry_ignores(workspace: &Path) -> Gitignore {
        // the second path takes priority
        build_ignore(
            [
                loader::config_dir().join("filesentryignore"),
                workspace.join(".mitos/filesentryignore"),
            ],
            workspace,
        )
        .unwrap_or(Gitignore::empty())
    }

    fn is_ignored(
        ignores: &[impl Borrow<Gitignore>],
        path: &Path,
        is_dir: Option<bool>,
    ) -> Option<bool> {
        match is_dir {
            Some(is_dir) => {
                for ignore in ignores {
                    match ignore.borrow().matched(path, is_dir) {
                        ignore::Match::None => continue,
                        ignore::Match::Ignore(_) => return Some(true),
                        ignore::Match::Whitelist(_) => return Some(false),
                    }
                }
            }
            None => {
                // if we don't know whether this is a directory (on windows)
                // then we are conservative and allow the dirs
                for ignore in ignores {
                    match ignore.borrow().matched(path, true) {
                        ignore::Match::None => continue,
                        ignore::Match::Ignore(glob) => {
                            if glob.is_only_dir() {
                                match ignore.borrow().matched(path, false) {
                                    ignore::Match::None => continue,
                                    ignore::Match::Ignore(_) => return Some(true),
                                    ignore::Match::Whitelist(_) => return Some(false),
                                }
                            } else {
                                return Some(true);
                            }
                        }
                        ignore::Match::Whitelist(_) => return Some(false),
                    }
                }
            }
        }
        None
    }
}

/// A filter for hidden and ignored files. The point of this
/// is to avoid overwhelming the watcher with watching a ton of
/// files/directories (like the cargo target directory, node_modules or
/// VCS files) so ignoring a file is a performance optimization.
pub(super) struct WatchFilter {
    filesentry_ignores: Gitignore,
    ignore_files: Vec<IgnoreFiles>,
    global_ignores: Vec<Arc<Gitignore>>,
    hidden: bool,
    watch_vcs: bool,
    max_depth: Option<usize>,
    config: Config,
    workspace_ignore: Option<Arc<Gitignore>>,
    nested_ignores:
        parking_lot::RwLock<std::collections::HashMap<PathBuf, Arc<Vec<Arc<Gitignore>>>>>,
}

impl WatchFilter {
    pub(super) fn new<'a>(
        config: &Config,
        workspace: &'a Path,
        roots: impl Iterator<Item = &'a Path> + Clone,
    ) -> WatchFilter {
        let filesentry_ignores = IgnoreFiles::filesentry_ignores(workspace);
        let (global_ignores, workspace_ignore) = IgnoreFiles::shared_ignores(workspace, config);
        let ignore_files = roots
            .chain([workspace])
            .map(|root| IgnoreFiles::new(workspace_ignore.clone(), config, root, &global_ignores))
            .collect();
        WatchFilter {
            filesentry_ignores,
            ignore_files,
            global_ignores,
            hidden: config.hidden,
            watch_vcs: config.watch_vcs,
            max_depth: config.max_depth,
            config: config.clone(),
            workspace_ignore,
            nested_ignores: Default::default(),
        }
    }

    fn directory_ignores(&self, path: &Path, root: &Path) -> Option<Arc<Vec<Arc<Gitignore>>>> {
        let parent = path.parent()?;
        if parent == root || !parent.starts_with(root) {
            return None;
        }
        if let Some(ignores) = self.nested_ignores.read().get(parent) {
            return Some(ignores.clone());
        }
        let ignores = Arc::new(
            IgnoreFiles::new(
                self.workspace_ignore.clone(),
                &self.config,
                parent,
                &self.global_ignores,
            )
            .ignores,
        );
        self.nested_ignores
            .write()
            .insert(parent.to_path_buf(), ignores.clone());
        Some(ignores)
    }

    fn ignore_path_impl(
        &self,
        path: &Path,
        is_dir: Option<bool>,
        ignore_files: &[Arc<Gitignore>],
    ) -> bool {
        if let Some(ignore) =
            IgnoreFiles::is_ignored(slice::from_ref(&self.filesentry_ignores), path, is_dir)
        {
            return ignore;
        }
        if is_hardcoded_whitelist(path) {
            return false;
        }
        if is_hardcoded_blacklist(path, is_dir.unwrap_or(false)) {
            return true;
        }
        if let Some(ignore) = IgnoreFiles::is_ignored(ignore_files, path, is_dir) {
            return ignore;
        }
        // ignore .git directory except .git/HEAD (and .git itself)
        if is_vcs_ignore(path, self.watch_vcs) {
            return true;
        }
        self.hidden && is_hidden(path)
    }
}

impl filesentry::Filter for WatchFilter {
    fn ignore_path(&self, path: &Path, is_dir: Option<bool>) -> bool {
        let (root, ignore_files) = self
            .ignore_files
            .iter()
            .filter(|files| path.starts_with(&files.root))
            .max_by_key(|files| files.root.components().count())
            .map_or((Path::new(""), &self.global_ignores), |files| {
                (&files.root, &files.ignores)
            });
        if path == root {
            return false;
        }
        if self.max_depth.is_some_and(|depth| {
            path.strip_prefix(root)
                .is_ok_and(|p| p.components().count() > depth)
        }) {
            return true;
        }
        let nested = self.directory_ignores(path, root);
        self.ignore_path_impl(
            path,
            is_dir,
            nested.as_deref().map(Vec::as_slice).unwrap_or(ignore_files),
        )
    }

    fn ignore_path_rec(&self, mut path: &Path, mut is_dir: Option<bool>) -> bool {
        let (root, ignore_files) = self
            .ignore_files
            .iter()
            .filter(|files| path.starts_with(&files.root))
            .max_by_key(|files| files.root.components().count())
            .map_or((Path::new(""), &self.global_ignores), |files| {
                (&files.root, &files.ignores)
            });
        if self.max_depth.is_some_and(|depth| {
            path.strip_prefix(root)
                .is_ok_and(|p| p.components().count() > depth)
        }) {
            return true;
        }
        loop {
            if path == root {
                return false;
            }
            let nested = self.directory_ignores(path, root);
            if self.ignore_path_impl(
                path,
                is_dir,
                nested.as_deref().map(Vec::as_slice).unwrap_or(ignore_files),
            ) {
                return true;
            }
            let Some(parent) = path.parent() else {
                break;
            };
            path = parent;
            // Ancestors of the leaf are always directories. Passing the leaf's
            // `is_dir` up the chain makes a dir-only pattern (gitignore `target/`)
            // miss the ancestor directory it names.
            is_dir = Some(true);
        }
        false
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|it| {
        it.as_encoded_bytes().first() == Some(&b'.')
        // handled by vcs ignore rules
        && it != ".git"
    })
}

// hidden directories we want to watch by default
fn is_hardcoded_whitelist(path: &Path) -> bool {
    path.ends_with(".gitignore")
        | path.ends_with(".ignore")
        | path.ends_with(".mitos")
        | path.ends_with(".github")
        | path.ends_with(".cargo")
        | path.ends_with(".envrc")
}

fn is_hardcoded_blacklist(path: &Path, is_dir: bool) -> bool {
    // don't descend into the cargo registry and similar
    path.parent()
        .is_some_and(|parent| parent.ends_with(".cargo"))
        && is_dir
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|it| it.to_str())
}

fn is_vcs_ignore(path: &Path, watch_vcs: bool) -> bool {
    // ignore .git directory contents except .git/HEAD (and .git itself)
    // Note: only checks immediate parent; recursive checking is done by ignore_path_rec
    if watch_vcs
        && path.parent().is_some_and(|it| it.ends_with(".git"))
        && !path.ends_with(".git/HEAD")
    {
        return true;
    }
    match file_name(path) {
        Some(".jj" | ".svn" | ".hg") => true,
        Some(".git") => !watch_vcs,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_hardcoded_whitelist, is_hidden, is_vcs_ignore};

    #[test]
    fn test_vcs_ignore() {
        assert!(!is_vcs_ignore(Path::new(".git"), true));
        assert!(!is_vcs_ignore(Path::new(".git/HEAD"), true));
        assert!(is_vcs_ignore(Path::new(".git/foo"), true));
        // Note: .git/foo/bar is NOT caught by is_vcs_ignore (only checks immediate parent)
        // but it IS caught by ignore_path_rec which checks ancestors recursively
        assert!(!is_vcs_ignore(Path::new(".git/foo/bar"), true));
        assert!(!is_vcs_ignore(Path::new(".foo"), true));
        assert!(is_vcs_ignore(Path::new(".jj"), true));
        assert!(is_vcs_ignore(Path::new(".svn"), true));
        assert!(is_vcs_ignore(Path::new(".hg"), true));
    }

    #[test]
    fn test_hidden() {
        assert!(is_hidden(Path::new(".foo")));
        // handled by vcs ignore rules
        assert!(!is_hidden(Path::new(".git")));
    }

    #[test]
    fn test_whitelist() {
        // Note: .git is NOT in whitelist - it has special handling in is_vcs_ignore and is_hidden
        assert!(is_hardcoded_whitelist(Path::new(".mitos")));
        assert!(is_hardcoded_whitelist(Path::new(".github")));
        assert!(!is_hardcoded_whitelist(Path::new(".githup")));
    }

    #[test]
    fn ignore_path_rec_treats_ancestors_as_dirs() {
        use std::sync::Arc;

        use filesentry::Filter;
        use ignore::gitignore::{Gitignore, GitignoreBuilder};

        use super::{Config, IgnoreFiles, WatchFilter};

        let mut builder = GitignoreBuilder::new("/repo");
        // dir-only pattern: matches the `target` directory, not a file named target
        builder.add_line(None, "target/").unwrap();
        let ignores = builder.build().unwrap();
        let filter = WatchFilter {
            filesentry_ignores: Gitignore::empty(),
            ignore_files: vec![IgnoreFiles {
                root: "/repo".into(),
                ignores: vec![Arc::new(ignores)],
            }],
            global_ignores: Vec::new(),
            hidden: true,
            watch_vcs: true,
            max_depth: None,
            config: Config::default(),
            workspace_ignore: None,
            nested_ignores: Default::default(),
        };

        // A file under `target/` is ignored even though the leaf is passed as a file:
        // the walk must re-check the `target` ancestor as a directory.
        assert!(filter.ignore_path_rec(Path::new("/repo/target/foo.rs"), Some(false)));
        assert!(filter.ignore_path_rec(Path::new("/repo/target/deep/foo.rs"), Some(false)));
        assert!(!filter.ignore_path_rec(Path::new("/repo/src/main.rs"), Some(false)));
    }
    #[test]
    fn filters_nested_ignores_hidden_paths_depth_and_independent_roots() {
        use super::{Config, WatchFilter};
        use filesentry::Filter;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/.gitignore"), "generated/\n").unwrap();
        let other = tempfile::tempdir().unwrap();
        let other = other.path().canonicalize().unwrap();
        let config = Config {
            git_global: false,
            max_depth: Some(3),
            ..Config::default()
        };
        let filter = WatchFilter::new(&config, &root, [other.as_path()].into_iter());
        assert!(filter.ignore_path_rec(&root.join("src/generated/file.rs"), Some(false)));
        assert!(filter.ignore_path_rec(&root.join(".hidden/file.rs"), Some(false)));
        assert!(filter.ignore_path_rec(&root.join("a/b/c/deep.rs"), Some(false)));
        assert!(!filter.ignore_path_rec(&root.join("src/main.rs"), Some(false)));
        assert!(!filter.ignore_path_rec(&other.join("src/generated/file.rs"), Some(false)));
        assert!(!filter.ignore_path_rec(&root.join(".mitos/config.toml"), Some(false)));
    }
}
