//! Dispatch for compiled VCS backends. Add backend variants here and register them in order.

use crate::{
    ChangedFileScope, Comparison, ComparisonKind, DocumentVcs, FileStatus, RepositoryWatches,
};
use anyhow::Result;
#[cfg(test)]
use arc_swap::ArcSwap;
use std::path::Path;
#[cfg(test)]
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) enum DiffProvider {
    #[cfg(feature = "git")]
    Git,
    #[cfg(test)]
    Test(Option<&'static str>),
}

impl DiffProvider {
    pub(super) fn inspect_file(
        &self,
        _file: &Path,
        _trust_full: bool,
        _read_diff: bool,
    ) -> Result<DocumentVcs> {
        match *self {
            #[cfg(feature = "git")]
            Self::Git => crate::git::inspect_file(_file, _trust_full, _read_diff),
            #[cfg(test)]
            Self::Test(name) => {
                let name = test_name(name)?;
                Ok(DocumentVcs {
                    diff_base: _read_diff.then(|| name.as_bytes().to_vec()),
                    head_name: Some(Arc::new(ArcSwap::from_pointee(name.into()))),
                })
            }
        }
    }

    pub(super) fn repository_watches(
        &self,
        _path: &Path,
        _trust_full: bool,
    ) -> Result<RepositoryWatches> {
        match *self {
            #[cfg(feature = "git")]
            Self::Git => crate::repository::repository_watches(_path, _trust_full),
            #[cfg(test)]
            Self::Test(name) => {
                let name = test_name(name)?;
                Ok(RepositoryWatches {
                    paths: vec![name.into()],
                    files: vec![name.into()],
                })
            }
        }
    }

    pub(super) fn for_each_file_status(
        &self,
        _scope: &ChangedFileScope,
        _trust_full: bool,
        _f: impl Fn(&Path, Result<FileStatus>) -> bool,
    ) -> Result<()> {
        match *self {
            #[cfg(feature = "git")]
            Self::Git => crate::git::for_each_file_status(_scope, _trust_full, _f),
            #[cfg(test)]
            Self::Test(name) => {
                let name = test_name(name)?;
                // An empty listing is still a successful provider response.
                if !name.is_empty() {
                    _f(
                        _scope.path(),
                        Ok(crate::FileStatus {
                            staged: None,
                            unstaged: Some(crate::FileChange::Modified {
                                path: _scope.path().join(name),
                            }),
                        }),
                    );
                }
                Ok(())
            }
        }
    }

    pub(super) fn read_comparison(
        &self,
        _status: &FileStatus,
        _kind: ComparisonKind,
        _trust_full: bool,
    ) -> Result<Comparison> {
        match *self {
            #[cfg(feature = "git")]
            Self::Git => crate::review::read_git_comparison(_status, _kind, _trust_full),
            #[cfg(test)]
            Self::Test(name) => Ok(Comparison {
                before: test_name(name)?.as_bytes().to_vec(),
                after: Vec::new(),
            }),
        }
    }

    pub(super) fn file_status(
        &self,
        _path: &Path,
        _trust_full: bool,
    ) -> Result<Option<FileStatus>> {
        match *self {
            #[cfg(feature = "git")]
            Self::Git => {
                let directory = _path
                    .parent()
                    .and_then(|path| path.ancestors().find(|path| path.is_dir()))
                    .ok_or_else(|| anyhow::anyhow!("File has no existing parent directory"))?;
                let result = std::cell::RefCell::new(None);
                crate::git::for_each_file_status(
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
            #[cfg(test)]
            Self::Test(name) => {
                let name = test_name(name)?;
                Ok((!name.is_empty()).then(|| crate::FileStatus {
                    staged: None,
                    unstaged: Some(crate::FileChange::Modified { path: _path.into() }),
                }))
            }
        }
    }
}

#[cfg(test)]
fn test_name(name: Option<&'static str>) -> Result<&'static str> {
    name.ok_or_else(|| anyhow::anyhow!("test provider unavailable"))
}
