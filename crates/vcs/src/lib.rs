//! Version-control status and asynchronous document diffs.
//!
//! Decorations are best-effort: an unavailable repository removes VCS decoration
//! instead of preventing a document from opening. Explicit status and comparison
//! operations report errors. Git support is selected at compile time; every
//! repository operation retains its explicit workspace trust decision.

use anyhow::Result;
use std::path::{Path, PathBuf};

#[cfg(feature = "git")]
mod git;

mod blame;
pub use blame::{FileBlame, LineBlame};

mod diff;

pub use diff::{DiffHandle, Hunk};

mod status;

pub use status::{FileChange, FileStatus};

mod review;
pub use review::{Comparison, ComparisonKind, MAX_REVIEW_BYTES};

#[cfg(feature = "git")]
pub mod operations;

/// Selects which changed files a status query should return.
#[derive(Clone)]
pub enum ChangedFileScope {
    /// Return only changes beneath this directory.
    Directory(PathBuf),
    /// Return every change in the repository containing this path.
    Repository(PathBuf),
}

impl ChangedFileScope {
    /// Returns the path used to discover the repository for this query.
    pub fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::Repository(path) => path,
        }
    }
}

/// Git metadata whose changes invalidate status, branch names, and diff bases.
pub fn get_watched_paths(path: &Path, trust_full: bool) -> Vec<PathBuf> {
    #[cfg(feature = "git")]
    {
        git::get_watched_paths(path, trust_full).unwrap_or_default()
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (path, trust_full);
        Vec::new()
    }
}

/// Read a document's unedited diff base, logging and suppressing repository errors.
pub fn get_diff_base(file: &Path, trust_full: bool) -> Option<Vec<u8>> {
    #[cfg(feature = "git")]
    {
        git::get_diff_base(file, trust_full)
            .inspect_err(|err| {
                log::debug!("failed to open diff base for {}: {err:#}", file.display())
            })
            .ok()
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (file, trust_full);
        None
    }
}

/// Read the current branch or detached HEAD name. Refreshes replace this snapshot.
pub fn get_current_head_name(file: &Path, trust_full: bool) -> Option<std::sync::Arc<str>> {
    #[cfg(feature = "git")]
    {
        git::get_current_head_name(file, trust_full)
            .inspect_err(|err| {
                log::debug!("failed to obtain head name for {}: {err:#}", file.display())
            })
            .ok()
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (file, trust_full);
        None
    }
}

/// Enumerate staged and working-tree status on a blocking worker.
/// Returning `false` from the callback stops iteration. Repository failures are
/// delivered once through the same callback at the query's discovery path.
pub fn for_each_file_status(
    scope: ChangedFileScope,
    trust_full: bool,
    f: impl Fn(&Path, Result<FileStatus>) -> bool + Send + 'static,
) {
    tokio::task::spawn_blocking(move || {
        #[cfg(feature = "git")]
        let result = git::for_each_file_status(&scope, trust_full, &f);
        #[cfg(not(feature = "git"))]
        let result: Result<()> = Err(anyhow::anyhow!("No Git support compiled in"));
        #[cfg(not(feature = "git"))]
        let _ = trust_full;
        if let Err(error) = result {
            f(scope.path(), Err(error));
        }
    });
}

/// Read bounded, immutable comparison inputs on the caller's blocking worker.
pub fn read_comparison(
    status: &FileStatus,
    kind: ComparisonKind,
    trust_full: bool,
) -> Result<Comparison> {
    #[cfg(feature = "git")]
    {
        review::read_git_comparison(status, kind, trust_full)
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (status, kind, trust_full);
        anyhow::bail!("No Git support compiled in")
    }
}

/// Resolve current staged/unstaged paths, including rename chains.
/// Call on a blocking worker; preview polling can reuse an existing status.
pub fn file_status(path: &Path, trust_full: bool) -> Result<Option<FileStatus>> {
    #[cfg(feature = "git")]
    {
        git::file_status(path, trust_full)
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (path, trust_full);
        anyhow::bail!("No Git support compiled in")
    }
}

#[cfg(all(test, not(feature = "git")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_git_skips_decorations_and_reports_explicit_operations() {
        let path = PathBuf::from("unavailable.txt");
        assert!(get_watched_paths(&path, false).is_empty());
        assert!(get_diff_base(&path, false).is_none());
        assert!(get_current_head_name(&path, false).is_none());
        assert!(file_status(&path, false).is_err());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for_each_file_status(
            ChangedFileScope::Repository(path.clone()),
            false,
            move |root, result| {
                tx.send((root.to_owned(), result)).unwrap();
                true
            },
        );
        let (root, result) = rx.recv().await.unwrap();
        assert_eq!(root, path);
        assert_eq!(
            result.unwrap_err().to_string(),
            "No Git support compiled in"
        );
        assert!(rx.recv().await.is_none());
    }
}
