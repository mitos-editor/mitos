//! A captured staging area and a bounded commit message, executed by Git itself.

use super::*;
use std::{cell::Cell, io::Write};

#[derive(Debug)]
pub(super) struct CommitFiles {
    message: tempfile::NamedTempFile,
    index: tempfile::TempPath,
    expected_index: Vec<u8>,
    _directory: tempfile::TempDir,
}

impl CommitFiles {
    pub(super) fn path(&self) -> &Path {
        self.message.path()
    }
    pub(super) fn index_path(&self) -> &Path {
        &self.index
    }

    /// Update the real index only while holding Git's lock and only if nobody staged newer work.
    pub(super) fn reconcile(&self, repository: &GitRepository) -> Option<String> {
        let reconcile = || -> Result<bool> {
            let path = repository.git_dir.join("index");
            let permissions = std::fs::metadata(&path)?.permissions();
            let mut lock = gix::lock::File::acquire_to_update_resource_with_permissions(
                &path,
                gix::lock::acquire::Fail::Immediately,
                None,
                || permissions.clone(),
            )?;
            if std::fs::read(&path)? != self.expected_index {
                return Ok(false);
            }
            let updated = std::fs::read(self.index_path())?;
            if updated == self.expected_index {
                return Ok(true);
            }
            lock.write_all(&updated)?;
            lock.flush()?;
            lock.close()?.commit()?;
            Ok(true)
        };
        match reconcile() {
            Ok(true) => None,
            Ok(false) => {
                Some("Newer staging was kept; review staged changes before the next commit".into())
            }
            Err(error) => {
                log::warn!(
                    "Commit attempt finished but the staging area could not be refreshed: {error:#}"
                );
                Some("Staging area was kept unchanged because its index could not be locked or refreshed; review staged changes".into())
            }
        }
    }
}

/// The staging area shown while composing a message. Changing the index, HEAD,
/// or repository configuration requires another review before committing.
#[derive(Clone, Debug)]
pub struct CommitSnapshot {
    repository: GitRepository,
    expected: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    index: Vec<u8>,
}

impl CommitSnapshot {
    pub fn repository(&self) -> &GitRepository {
        &self.repository
    }

    /// Run on a blocking worker. The message file is private and removed when
    /// the prepared operation finishes or is dropped, including failures.
    pub fn prepare(&self, message: &str, trusted: bool) -> Result<PreparedOperation> {
        ensure!(
            trusted,
            "Git commits require workspace trust; run :workspace-trust first"
        );
        ensure!(
            !message.trim().is_empty(),
            "Write a commit message before submitting"
        );
        ensure!(
            message.len() <= 1024 * 1024,
            "Commit message exceeds the 1 MiB limit"
        );
        self.repository.open(true)?;
        ensure!(
            self.expected
                == self
                    .repository
                    .stamp(self.expected.iter().map(|(path, _)| path.clone()))
                && self.index == std::fs::read(self.repository.git_dir.join("index"))?,
            "The staging area changed while composing; run :vcs-commit to review it again"
        );
        let directory = tempfile::tempdir()?;
        let mut file = tempfile::NamedTempFile::new_in(directory.path())?;
        file.write_all(message.as_bytes())?;
        file.flush()?;
        let mut index = tempfile::NamedTempFile::new_in(directory.path())?;
        index.write_all(&self.index)?;
        index.flush()?;
        // Close the handle before Git replaces the index, including on Windows.
        let index = index.into_temp_path();
        Ok(PreparedOperation {
            repository: self.repository.clone(),
            operation: Operation::Commit(CommitFiles {
                message: file,
                index,
                expected_index: self.index.clone(),
                _directory: directory,
            }),
            expected: self.expected.clone(),
        })
    }
}

impl GitRepository {
    /// Capture the nonempty, conflict-free staging area on a blocking worker.
    pub fn commit_snapshot(&self, trusted: bool) -> Result<CommitSnapshot> {
        ensure!(
            trusted,
            "Git commits require workspace trust; run :workspace-trust first"
        );
        let repo = self.open(true)?;
        let mut watched = crate::git::repository_watched_paths(&repo)?;
        watched.extend(
            [
                "MERGE_HEAD",
                "CHERRY_PICK_HEAD",
                "REVERT_HEAD",
                "MERGE_MODE",
                "MERGE_MSG",
                "SQUASH_MSG",
            ]
            .map(|name| self.git_dir.join(name)),
        );
        let expected = self.stamp(watched);
        ensure!(
            repo.index_or_empty()?
                .entries()
                .iter()
                .all(|entry| { entry.stage() == gix::index::entry::Stage::Unconflicted }),
            "Resolve index conflicts before committing"
        );
        let staged = Cell::new(false);
        let error = std::cell::RefCell::new(None);
        crate::git::for_each_file_status(
            &crate::ChangedFileScope::Repository(self.root.clone()),
            true,
            |_, status| match status {
                Ok(status) => {
                    staged.set(staged.get() || status.staged.is_some());
                    !staged.get()
                }
                Err(err) => {
                    *error.borrow_mut() = Some(err);
                    false
                }
            },
        )?;
        if let Some(error) = error.into_inner() {
            return Err(error);
        }
        ensure!(
            staged.get(),
            "No staged changes; stage the changes you want to commit first"
        );
        let index = std::fs::read(self.git_dir.join("index"))?;
        ensure!(
            expected == self.stamp(expected.iter().map(|(path, _)| path.clone())),
            "Git state changed while preparing the commit; try again"
        );
        Ok(CommitSnapshot {
            repository: self.clone(),
            expected,
            index,
        })
    }
}
