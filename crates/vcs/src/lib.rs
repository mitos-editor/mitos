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

/// Best-effort access to the compiled Git backend.
#[derive(Clone, Default)]
pub struct DiffProviderRegistry {}

impl DiffProviderRegistry {
    /// Files whose changes invalidate VCS state without excluding neighboring source files.
    pub fn get_watched_files(&self, _path: &Path, _trust_full: bool) -> Vec<PathBuf> {
        #[cfg(feature = "git")]
        {
            git::get_watched_files(_path, _trust_full).unwrap_or_default()
        }
        #[cfg(not(feature = "git"))]
        {
            Vec::new()
        }
    }

    /// Git metadata whose changes invalidate status, branch names, and document diff bases.
    pub fn get_watched_paths(&self, _path: &Path, _trust_full: bool) -> Vec<PathBuf> {
        #[cfg(feature = "git")]
        {
            git::get_watched_paths(_path, _trust_full).unwrap_or_default()
        }
        #[cfg(not(feature = "git"))]
        {
            Vec::new()
        }
    }

    /// Read the unedited file, logging and suppressing errors for editor decoration.
    pub fn get_diff_base(&self, _file: &Path, _trust_full: bool) -> Option<Vec<u8>> {
        #[cfg(feature = "git")]
        {
            git::get_diff_base(_file, _trust_full)
                .inspect_err(|error| {
                    log::debug!("{error:#?}");
                    log::debug!("failed to open diff base for {}", _file.display());
                })
                .ok()
        }
        #[cfg(not(feature = "git"))]
        {
            None
        }
    }

    /// Return a live head name, logging and suppressing errors for editor decoration.
    pub fn get_current_head_name(
        &self,
        _file: &Path,
        _trust_full: bool,
    ) -> Option<Arc<ArcSwap<Box<str>>>> {
        #[cfg(feature = "git")]
        {
            git::get_current_head_name(_file, _trust_full)
                .inspect_err(|error| {
                    log::debug!("{error:#?}");
                    log::debug!("failed to obtain current head name for {}", _file.display());
                })
                .ok()
        }
        #[cfg(not(feature = "git"))]
        {
            None
        }
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

    /// Enumerate index and working-tree changes, reporting one terminal error on failure.
    pub fn for_each_file_status(
        self,
        scope: ChangedFileScope,
        _trust_full: bool,
        f: impl Fn(&Path, Result<FileStatus>) -> bool + Send + 'static,
    ) {
        tokio::task::spawn_blocking(move || {
            #[cfg(feature = "git")]
            if git::for_each_file_status(&scope, _trust_full, &f).is_ok() {
                return;
            }
            f(
                scope.path(),
                Err(anyhow!("no diff provider returns success")),
            );
        });
    }

    /// Read bounded comparison inputs on the caller's blocking worker.
    pub fn read_comparison(
        &self,
        _status: &FileStatus,
        _kind: ComparisonKind,
        _trust_full: bool,
    ) -> Result<Comparison> {
        #[cfg(feature = "git")]
        {
            review::read_git_comparison(_status, _kind, _trust_full)
        }
        #[cfg(not(feature = "git"))]
        {
            anyhow::bail!("No Git support compiled in")
        }
    }

    /// Resolve current staged/unstaged paths, including rename chains, on a blocking worker.
    pub fn file_status(&self, _path: &Path, _trust_full: bool) -> Result<Option<FileStatus>> {
        #[cfg(feature = "git")]
        {
            let directory = _path
                .parent()
                .and_then(|path| path.ancestors().find(|path| path.is_dir()))
                .ok_or_else(|| anyhow!("File has no existing parent directory"))?;
            let result = std::cell::RefCell::new(None);
            git::for_each_file_status(
                &ChangedFileScope::Repository(directory.into()),
                _trust_full,
                |_, status| match status {
                    Ok(status) if status.path() == _path => {
                        *result.borrow_mut() = Some(Ok(status));
                        false
                    }
                    Err(error) => {
                        *result.borrow_mut() = Some(Err(error));
                        false
                    }
                    _ => true,
                },
            )?;
            result.into_inner().transpose()
        }
        #[cfg(not(feature = "git"))]
        {
            anyhow::bail!("No Git support compiled in")
        }
    }
}

#[cfg(all(test, not(feature = "git")))]
#[tokio::test]
async fn no_git_reports_one_terminal_enumeration_error() {
    let path = Path::new("missing-repository");
    let (sender, mut responses) = tokio::sync::mpsc::unbounded_channel();
    DiffProviderRegistry::default().for_each_file_status(
        ChangedFileScope::Repository(path.into()),
        false,
        move |root, result| {
            sender
                .send((root.to_owned(), result.unwrap_err().to_string()))
                .unwrap();
            true
        },
    );
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), responses.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        response,
        (path.into(), "no diff provider returns success".into())
    );
    assert!(responses.recv().await.is_none());
}
