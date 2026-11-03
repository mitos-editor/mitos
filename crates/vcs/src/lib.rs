//! Version-control status and asynchronous document diffs.
//!
//! [`DiffProviderRegistry`] is deliberately best-effort: callers are editing
//! files, so an unavailable repository or provider should remove VCS decoration
//! rather than prevent the document from opening. Provider failures from
//! [`DiffProviderRegistry::inspect_file`] are logged and returned as absent
//! decoration. Changed-file enumeration reports a terminal error through its
//! callback when no provider can serve the query.
//!
//! Git is currently the only compiled provider. The `trust_full` argument on
//! registry operations controls whether repository-local configuration and the
//! features it enables may be trusted; preserve it when adding new entry points.

use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(feature = "git")]
mod git;

mod blame;
pub mod conflict;
pub use blame::{FileBlame, LineBlame};

mod provider;
use provider::DiffProvider;

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

/// Ordered access to the VCS providers compiled into this editor.
///
/// Watch inputs are combined across providers. Other queries use the first
/// provider that succeeds, including a successful empty status listing.
#[derive(Clone)]
pub struct DiffProviderRegistry {
    providers: Arc<[DiffProvider]>,
}

impl Default for DiffProviderRegistry {
    fn default() -> Self {
        Self {
            providers: vec![
                #[cfg(feature = "git")]
                DiffProvider::Git,
            ]
            .into(),
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

impl DiffProviderRegistry {
    /// Read one provider's independent decorations with one discovery and current trust/configuration.
    /// Binary documents skip blob reads and worktree filters.
    pub fn inspect_file(&self, file: &Path, trust_full: bool, read_diff: bool) -> DocumentVcs {
        self.providers
            .iter()
            .find_map(|provider| {
                provider
                    .inspect_file(file, trust_full, read_diff)
                    .inspect_err(|error| {
                        log::debug!("VCS inspection for {}: {error:#}", file.display())
                    })
                    .ok()
            })
            .unwrap_or_default()
    }

    /// Collect every provider's watch inputs, sharing discovery within each provider.
    pub fn repository_watches(&self, path: &Path, trust_full: bool) -> RepositoryWatches {
        let mut watches = RepositoryWatches::default();
        for provider in self.providers.iter() {
            if let Ok(inputs) = provider.repository_watches(path, trust_full) {
                watches.paths.extend(inputs.paths);
                watches.files.extend(inputs.files);
            }
        }
        watches
    }

    /// Enumerate changed files on a blocking worker. Returning false stops iteration.
    pub fn for_each_changed_file(
        self,
        scope: ChangedFileScope,
        trust_full: bool,
        f: impl Fn(&Path, Result<FileChange>) -> bool + Send + 'static,
    ) {
        self.for_each_file_status(scope, trust_full, move |root, status| {
            f(root, status.map(|status| status.change().clone()))
        });
    }

    /// Enumerate with the first successful provider, reporting one terminal error if none succeeds.
    pub fn for_each_file_status(
        self,
        scope: ChangedFileScope,
        trust_full: bool,
        f: impl Fn(&Path, Result<FileStatus>) -> bool + Send + 'static,
    ) {
        tokio::task::spawn_blocking(move || {
            if self
                .query(|provider| provider.for_each_file_status(&scope, trust_full, &f))
                .is_err()
            {
                f(
                    scope.path(),
                    Err(anyhow!("no diff provider returns success")),
                );
            }
        });
    }

    /// Read bounded comparison inputs on the caller's blocking worker.
    pub fn read_comparison(
        &self,
        status: &FileStatus,
        kind: ComparisonKind,
        trust_full: bool,
    ) -> Result<Comparison> {
        self.query(|provider| provider.read_comparison(status, kind, trust_full))
    }

    /// Resolve current staged/unstaged paths, including rename chains, on a blocking worker.
    pub fn file_status(&self, path: &Path, trust_full: bool) -> Result<Option<FileStatus>> {
        self.query(|provider| provider.file_status(path, trust_full))
    }

    fn query<T>(&self, mut query: impl FnMut(&DiffProvider) -> Result<T>) -> Result<T> {
        let mut error = anyhow!("No VCS provider available");
        for provider in self.providers.iter() {
            match query(provider) {
                Ok(result) => return Ok(result),
                Err(err) => error = err,
            }
        }
        Err(error)
    }
}

#[cfg(test)]
mod tests;
