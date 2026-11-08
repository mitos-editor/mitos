//! Optional Git workflows. Reads use captured repository paths; writes respect index locking
//! and clean-filter semantics. Frontends own authorization and document coordination.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::command::{run, run_private};
use anyhow::{bail, ensure, Context, Result};
use tokio::process::Command;

use crate::{FileChange, FileStatus};

pub mod history;
pub mod repository;
pub use repository::RepositoryOperation;
mod branch;
mod checkout;
mod fetch;
mod index;
mod stage;
mod stash;
mod unstage;
pub use stash::{StashEntry, StashFile, StashLayer};

pub use crate::repository::GitRepository;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOperation {
    Stage,
    Unstage,
}

impl FileOperation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage file on disk",
            Self::Unstage => "Unstage file",
        }
    }
}

#[derive(Clone, Debug)]
enum Operation {
    Repository(RepositoryOperation),
    File {
        kind: FileOperation,
        paths: Vec<PathBuf>,
        unborn: bool,
    },
    Stash,
    Apply {
        id: String,
        restore_index: bool,
    },
}

/// Immutable preflight. A changed index, HEAD, configuration, or selected file invalidates it.
pub struct PreparedOperation {
    repository: GitRepository,
    operation: Operation,
    expected: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
}

impl PreparedOperation {
    pub fn repository(&self) -> &GitRepository {
        &self.repository
    }

    pub fn changes_worktree(&self) -> bool {
        if let Operation::Repository(operation) = &self.operation {
            return operation.changes_worktree();
        }
        !matches!(
            self.operation,
            Operation::File {
                kind: FileOperation::Stage | FileOperation::Unstage,
                ..
            }
        )
    }

    /// File paths, or the entire worktree for stash application, whose writes must be suspended.
    pub fn affected_paths(&self) -> Vec<PathBuf> {
        match &self.operation {
            Operation::Repository(operation) => operation.affected_paths(&self.repository),
            Operation::File { paths, .. } => paths
                .iter()
                .map(|path| self.repository.root.join(path))
                .collect(),
            Operation::Stash | Operation::Apply { .. } => vec![self.repository.root.clone()],
        }
    }

    pub fn label(&self) -> &'static str {
        match &self.operation {
            Operation::Repository(operation) => operation.label(),
            Operation::File { kind, .. } => kind.label(),
            Operation::Stash => "Stash all repository changes",
            Operation::Apply { .. } => "Apply stash (keep stash)",
        }
    }

    fn ensure_current(&self) -> Result<()> {
        ensure!(
            self.expected
                == self
                    .repository
                    .stamp(self.expected.iter().map(|(path, _)| path.clone())),
            "Git state changed while preparing this action; refresh and try again"
        );
        Ok(())
    }

    /// Once started, a mutation must be driven to completion, even after its originating UI closes.
    pub async fn execute(self) -> Result<()> {
        let prepared = tokio::task::spawn_blocking(move || {
            self.ensure_current()?;
            if matches!(
                self.operation,
                Operation::File {
                    kind: FileOperation::Stage,
                    ..
                }
            ) && self.stage()?
            {
                return Ok(None);
            }
            if matches!(
                self.operation,
                Operation::File {
                    kind: FileOperation::Unstage,
                    ..
                }
            ) && self.unstage()?
            {
                return Ok(None);
            }
            match &self.operation {
                Operation::Repository(RepositoryOperation::CreateBranch { name }) => {
                    self.create_branch(name)?;
                    return Ok(None);
                }
                Operation::Repository(RepositoryOperation::DeleteBranch { name }) => {
                    self.delete_branch(name)?;
                    return Ok(None);
                }
                Operation::Repository(RepositoryOperation::SwitchBranch { name }) => {
                    self.switch_branch(name)?;
                    return Ok(None);
                }
                Operation::Repository(RepositoryOperation::CreateWorktree { path, branch }) => {
                    self.create_worktree(path, branch.as_deref())?;
                    return Ok(None);
                }
                Operation::Repository(RepositoryOperation::RemoveWorktree { path }) => {
                    self.remove_worktree(path)?;
                    return Ok(None);
                }
                _ => {}
            }
            Ok::<_, anyhow::Error>(Some(self))
        })
        .await??;
        let Some(prepared) = prepared else {
            return Ok(());
        };
        if let Operation::Repository(operation) = &prepared.operation {
            return operation.execute(&prepared.repository).await;
        }
        let mut command = prepared.repository.command();
        match &prepared.operation {
            Operation::Repository(_) => unreachable!(),
            Operation::File {
                kind: FileOperation::Stage,
                paths,
                ..
            } => {
                command
                    .args(["add", "--all", "--"])
                    .args(paths.iter().map(literal_pathspec));
            }
            Operation::File {
                kind: FileOperation::Unstage,
                paths,
                unborn,
            } => {
                if *unborn {
                    command
                        .args(["rm", "--cached", "--force", "--ignore-unmatch", "--"])
                        .args(paths.iter().map(literal_pathspec));
                } else {
                    command
                        .args(["reset", "--quiet", "HEAD", "--"])
                        .args(paths.iter().map(literal_pathspec));
                }
            }
            Operation::Stash => {
                command.args([
                    "stash",
                    "push",
                    "--include-untracked",
                    "--message",
                    "Mitos: repository changes",
                ]);
            }
            Operation::Apply { id, restore_index } => {
                command.args(["stash", "apply"]);
                if *restore_index {
                    command.arg("--index");
                }
                command.arg(id);
            }
        }
        run(command).await.map(|_| ())
    }
}

impl GitRepository {
    pub fn prepare_file(
        &self,
        status: &FileStatus,
        kind: FileOperation,
        trusted: bool,
    ) -> Result<PreparedOperation> {
        ensure!(
            trusted,
            "Git actions require workspace trust; run :workspace-trust first"
        );

        let repo = match kind {
            FileOperation::Unstage => self.open_read(true)?,
            FileOperation::Stage => self.open(true)?,
        };
        let change = match kind {
            FileOperation::Stage => status
                .unstaged
                .as_ref()
                .context("This file has no unstaged changes")?,
            FileOperation::Unstage => status
                .staged
                .as_ref()
                .context("This file has no staged changes")?,
        };
        let mut paths = Vec::with_capacity(2);
        if let FileChange::Renamed { from_path, .. } = change {
            paths.push(self.relative(from_path)?);
        }
        paths.push(self.relative(change.path())?);
        paths.sort();
        paths.dedup();
        ensure!(!paths.is_empty(), "No file changes selected");
        let index = repo.index_or_empty()?;
        for path in &paths {
            let bytes = gix::path::into_bstr(path)?;
            if let Some(entry) = index
                .entry_by_path_and_stage(bytes.as_ref(), gix::index::entry::Stage::Unconflicted)
            {
                ensure!(
                    entry.mode != gix::index::entry::Mode::COMMIT,
                    "Submodule actions are not supported"
                );
            }
            ensure!(
                !std::fs::symlink_metadata(self.root.join(path))
                    .is_ok_and(|metadata| metadata.is_dir()),
                "Git actions require a file, not a directory"
            );
        }
        let mut watched = crate::repository::repository_watched_paths(&repo)?;
        for path in &paths {
            let full = self.root.join(path);
            watched.extend(
                full.parent()
                    .into_iter()
                    .flat_map(|parent| parent.ancestors())
                    .map(|parent| parent.join(".gitattributes")),
            );
            watched.push(full);
        }
        let expected = self.stamp(watched);
        Ok(PreparedOperation {
            repository: self.clone(),
            operation: Operation::File {
                kind,
                paths,
                unborn: repo.head()?.is_unborn(),
            },
            expected,
        })
    }

    pub fn prepare_stash(&self, trusted: bool) -> Result<PreparedOperation> {
        ensure!(
            trusted,
            "Git actions require workspace trust; run :workspace-trust first"
        );
        let repo = self.open(true)?;
        ensure!(
            !repo.head()?.is_unborn(),
            "Create the first commit before stashing changes"
        );
        let index = repo.index_or_empty()?;
        ensure!(
            index
                .entries()
                .iter()
                .all(|entry| entry.stage() == gix::index::entry::Stage::Unconflicted),
            "Resolve index conflicts before stashing"
        );
        let expected = self.stamp(crate::repository::repository_watched_paths(&repo)?);
        Ok(PreparedOperation {
            repository: self.clone(),
            operation: Operation::Stash,
            expected,
        })
    }

    pub fn prepare_apply(
        &self,
        entry: &StashEntry,
        restore_index: bool,
        trusted: bool,
    ) -> Result<PreparedOperation> {
        ensure!(
            trusted,
            "Git actions require workspace trust; run :workspace-trust first"
        );

        let repo = self.open(true)?;
        stash::trees(&repo, &entry.id)?;
        let expected = self.stamp(crate::repository::repository_watched_paths(&repo)?);
        Ok(PreparedOperation {
            repository: self.clone(),
            operation: Operation::Apply {
                id: entry.id.clone(),
                restore_index,
            },
            expected,
        })
    }

    fn relative(&self, path: &Path) -> Result<PathBuf> {
        let parent = path.parent().context("File has no parent")?;
        let path = stdx::path::canonicalize_existing(parent)
            .join(path.file_name().context("File has no name")?);
        let relative = path
            .strip_prefix(&self.root)
            .context("Selected file is outside this Git working tree")?;
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_))),
            "Invalid Git file path"
        );
        ensure!(
            !relative.components().any(|part| part.as_os_str() == ".git"),
            "Git metadata cannot be selected for file actions"
        );
        Ok(relative.to_owned())
    }

    fn stamp(
        &self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Vec<(PathBuf, Option<(SystemTime, u64)>)> {
        let mut paths: Vec<_> = paths.into_iter().collect();
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| {
                let stamp = std::fs::symlink_metadata(&path)
                    .ok()
                    .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));
                (path, stamp)
            })
            .collect()
    }

    fn command(&self) -> Command {
        Command::from(crate::command::repository(
            &self.root,
            &self.git_dir,
            crate::command::Access::Mutation,
        ))
    }
}

// Use literal magic on each user path. The global --literal-pathspecs option also reaches
// Git's internal stash/clean subprocesses and can prevent untracked-file cleanup.
fn literal_pathspec(path: &PathBuf) -> std::ffi::OsString {
    let mut argument = std::ffi::OsString::from(":(literal)");
    argument.push(path);
    argument
}

fn literal(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .take(1000)
        .collect()
}

#[cfg(test)]
mod tests;
