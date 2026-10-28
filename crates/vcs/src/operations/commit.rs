//! A captured staging area and a bounded commit message, executed by Git itself.

use super::*;
use std::{cell::Cell, io::Write};

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
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(message.as_bytes())?;
        file.flush()?;
        Ok(PreparedOperation {
            repository: self.repository.clone(),
            operation: Operation::Commit(file),
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
        let expected = self.stamp(crate::git::repository_watched_paths(&repo)?);
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
