//! Version-control status and asynchronous document diffs.
//!
//! [`DiffProviderRegistry`] is deliberately best-effort: callers are editing
//! files, so an unavailable repository or provider should remove VCS decoration
//! rather than prevent the document from opening. Provider failures from
//! [`DiffProviderRegistry::get_diff_base`] and
//! [`DiffProviderRegistry::get_current_head_name`] are logged and returned as
//! `None`. Changed-file enumeration reports a terminal error through its
//! callback when no provider can serve the query.
//!
//! Git is currently the only compiled provider. The `trust_full` argument on
//! registry operations controls whether repository-local configuration and the
//! features it enables may be trusted; preserve it when adding new entry points.

use anyhow::{anyhow, bail, Result};
use arc_swap::ArcSwap;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(feature = "git")]
mod git;

mod blame;
pub use blame::FileBlame;

mod diff;

pub use diff::{DiffHandle, Hunk};

mod status;

pub use status::FileChange;

/// Selects which changed files a status query should return.
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

/// Contains all active diff providers. Diff providers are compiled in via features. Currently
/// only `git` is supported.
#[derive(Clone)]
pub struct DiffProviderRegistry {
    providers: Vec<DiffProvider>,
}

impl DiffProviderRegistry {
    /// Git metadata whose changes invalidate branch names and document diff bases.
    pub fn get_watched_paths(&self, path: &Path, trust_full: bool) -> Vec<PathBuf> {
        self.providers
            .iter()
            .flat_map(|provider| match provider {
                #[cfg(feature = "git")]
                DiffProvider::Git => git::get_watched_paths(path, trust_full).unwrap_or_default(),
                DiffProvider::None => Vec::new(),
            })
            .collect()
    }

    /// Reads the unedited version of `file` used as the base of a document diff.
    ///
    /// Providers are tried in registry order. Errors are logged and suppressed;
    /// `None` means no provider produced a base.
    pub fn get_diff_base(&self, file: &Path, trust_full: bool) -> Option<Vec<u8>> {
        self.providers
            .iter()
            .find_map(|provider| match provider.get_diff_base(file, trust_full) {
                Ok(res) => Some(res),
                Err(err) => {
                    log::debug!("{err:#?}");
                    log::debug!("failed to open diff base for {}", file.display());
                    None
                }
            })
    }

    /// Returns a live, shareable value containing the current repository head name.
    ///
    /// The inner [`ArcSwap`] may be updated by the provider after branch changes.
    /// Provider errors are logged and suppressed.
    pub fn get_current_head_name(
        &self,
        file: &Path,
        trust_full: bool,
    ) -> Option<Arc<ArcSwap<Box<str>>>> {
        self.providers.iter().find_map(|provider| {
            match provider.get_current_head_name(file, trust_full) {
                Ok(res) => Some(res),
                Err(err) => {
                    log::debug!("{err:#?}");
                    log::debug!("failed to obtain current head name for {}", file.display());
                    None
                }
            }
        })
    }

    /// Starts changed-file enumeration on a blocking worker thread.
    ///
    /// The callback may run repeatedly on that worker and returning `false`
    /// stops iteration. If no provider succeeds, it is called once with an error
    /// and [`ChangedFileScope::path`] as the path.
    pub fn for_each_changed_file(
        self,
        scope: ChangedFileScope,
        trust_full: bool,
        f: impl Fn(&Path, Result<FileChange>) -> bool + Send + 'static,
    ) {
        tokio::task::spawn_blocking(move || {
            if self
                .providers
                .iter()
                .find_map(|provider| provider.for_each_changed_file(&scope, trust_full, &f).ok())
                .is_none()
            {
                f(
                    scope.path(),
                    Err(anyhow!("no diff provider returns success")),
                );
            }
        });
    }
}

impl Default for DiffProviderRegistry {
    fn default() -> Self {
        // currently only git is supported
        // TODO make this configurable when more providers are added
        let providers = vec![
            #[cfg(feature = "git")]
            DiffProvider::Git,
            DiffProvider::None,
        ];
        DiffProviderRegistry { providers }
    }
}

/// A union type that includes all types that implement [DiffProvider]. We need this type to allow
/// cloning [DiffProviderRegistry] as `Clone` cannot be used in trait objects.
///
/// `Copy` is simply to ensure the `clone()` call is the simplest it can be.
#[derive(Copy, Clone)]
enum DiffProvider {
    #[cfg(feature = "git")]
    Git,
    None,
}

impl DiffProvider {
    fn get_diff_base(&self, file: &Path, trust_full: bool) -> Result<Vec<u8>> {
        match self {
            #[cfg(feature = "git")]
            Self::Git => git::get_diff_base(file, trust_full),
            Self::None => bail!("No diff support compiled in"),
        }
    }

    fn get_current_head_name(
        &self,
        file: &Path,
        trust_full: bool,
    ) -> Result<Arc<ArcSwap<Box<str>>>> {
        match self {
            #[cfg(feature = "git")]
            Self::Git => git::get_current_head_name(file, trust_full),
            Self::None => bail!("No diff support compiled in"),
        }
    }

    fn for_each_changed_file(
        &self,
        scope: &ChangedFileScope,
        trust_full: bool,
        f: impl Fn(&Path, Result<FileChange>) -> bool,
    ) -> Result<()> {
        match self {
            #[cfg(feature = "git")]
            Self::Git => git::for_each_changed_file(scope, trust_full, f),
            Self::None => bail!("No diff support compiled in"),
        }
    }
}
