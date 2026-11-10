//! Version-control status and asynchronous document diffs.
//!
//! [`inspect_file`] is deliberately best-effort: callers are editing files,
//! so an unavailable repository should remove VCS decoration rather than prevent
//! the document from opening. Inspection failures are logged and returned as absent
//! decoration. Changed-file enumeration reports a terminal error through its
//! callback when the query cannot be served.
//!
//! The optional `git` feature enables repository operations. The `trust_full`
//! argument controls whether repository-local configuration and the features it
//! enables may be trusted; preserve it when adding new entry points.

use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(feature = "git")]
mod git;
#[cfg(feature = "git")]
pub mod repository;

#[cfg(feature = "git")]
mod blob;

mod blame;
pub mod conflict;
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

/// Decoration read together from one freshly opened repository.
#[derive(Default)]
pub struct DocumentVcs {
    pub diff_base: Option<Vec<u8>>,
    pub head_name: Option<Arc<ArcSwap<Box<str>>>>,
}

/// Repository inputs watched as metadata paths or individual source files.
#[derive(Default)]
pub struct RepositoryWatches {
    pub paths: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

fn collect_repository_watches<'a>(
    paths: impl IntoIterator<Item = (&'a Path, bool)>,
    mut inspect: impl FnMut(&Path, bool) -> RepositoryWatches,
) -> RepositoryWatches {
    let mut seen = std::collections::BTreeSet::new();
    let mut watches = RepositoryWatches::default();
    for (path, trusted) in paths {
        if seen.insert((path, trusted)) {
            let inputs = inspect(path, trusted);
            watches.paths.extend(inputs.paths);
            watches.files.extend(inputs.files);
        }
    }
    watches
}

/// Read decorations with one discovery and current trust/configuration.
/// Binary documents skip blob reads and worktree filters.
pub fn inspect_file(file: &Path, trust_full: bool, read_diff: bool) -> DocumentVcs {
    #[cfg(feature = "git")]
    {
        git::inspect_file(file, trust_full, read_diff)
            .inspect_err(|error| log::debug!("VCS inspection for {}: {error:#}", file.display()))
            .unwrap_or_default()
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (file, trust_full, read_diff);
        DocumentVcs::default()
    }
}

/// Read repository watch inputs, or an empty set when unavailable.
pub fn repository_watches(path: &Path, trust_full: bool) -> RepositoryWatches {
    #[cfg(feature = "git")]
    {
        repository::repository_watches(path, trust_full).unwrap_or_default()
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (path, trust_full);
        RepositoryWatches::default()
    }
}

/// Combine watch inputs for one refresh, inspecting each directory and trust pair once.
/// Entries never survive this call, so configuration and trust changes are read afresh.
pub fn repository_watches_for_paths<'a>(
    paths: impl IntoIterator<Item = (&'a Path, bool)>,
) -> RepositoryWatches {
    collect_repository_watches(paths, repository_watches)
}

/// Enumerate on a blocking worker, reporting one terminal error if the query fails.
pub fn for_each_file_status(
    scope: ChangedFileScope,
    trust_full: bool,
    f: impl Fn(&Path, Result<FileStatus>) -> bool + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        #[cfg(feature = "git")]
        let result = git::for_each_file_status(&scope, trust_full, &f);
        #[cfg(not(feature = "git"))]
        let result: Result<()> = {
            let _ = trust_full;
            Err(anyhow!("No VCS provider available"))
        };
        if result.is_err() {
            f(
                scope.path(),
                Err(anyhow!("no diff provider returns success")),
            );
        }
    })
}

/// Read bounded comparison inputs on the caller's blocking worker.
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
        Err(anyhow!("No VCS provider available"))
    }
}

/// Resolve current staged/unstaged paths, including rename chains, on a blocking worker.
pub fn file_status(path: &Path, trust_full: bool) -> Result<Option<FileStatus>> {
    #[cfg(feature = "git")]
    {
        git::file_status(path, trust_full)
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (path, trust_full);
        Err(anyhow!("No VCS provider available"))
    }
}

#[cfg(test)]
mod tests;
