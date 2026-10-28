//! Optional Git workflows. Reads use captured repository paths; writes use Git's index locking
//! and filter semantics, never a shell. Frontends own authorization and document coordination.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::SystemTime,
};

use anyhow::{bail, ensure, Context, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use crate::{FileChange, FileStatus};

mod commit;
pub mod history;
mod partial;
pub use commit::CommitSnapshot;
pub use partial::{PartialSelection, PartialSource};
mod stash;
pub use stash::{StashEntry, StashFile, StashLayer};

/// Paths are captured before any subprocess starts, including in linked worktrees.
#[derive(Clone, Debug)]
pub struct GitRepository {
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

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

#[derive(Debug)]
enum Operation {
    Partial {
        kind: FileOperation,
        path: PathBuf,
        patch: Vec<u8>,
        next_line: Option<u32>,
    },
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
    Commit(commit::CommitFiles),
}

/// Immutable preflight. A changed index, HEAD, configuration, or selected file invalidates it.
pub struct PreparedOperation {
    repository: GitRepository,
    operation: Operation,
    expected: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
}

impl PreparedOperation {
    pub fn next_partial_line(&self) -> Option<u32> {
        match &self.operation {
            Operation::Partial { next_line, .. } => *next_line,
            _ => None,
        }
    }
    pub fn repository(&self) -> &GitRepository {
        &self.repository
    }

    pub fn changes_worktree(&self) -> bool {
        !matches!(
            self.operation,
            Operation::Partial { .. }
                | Operation::File {
                    kind: FileOperation::Stage | FileOperation::Unstage,
                    ..
                }
        )
    }

    /// Commit hooks may update files, but unsaved editor text can be retained.
    pub fn requires_clean_buffers(&self) -> bool {
        self.changes_worktree() && !matches!(self.operation, Operation::Commit(_))
    }

    /// File paths, or the entire worktree for stash application, whose writes must be suspended.
    pub fn affected_paths(&self) -> Vec<PathBuf> {
        match &self.operation {
            Operation::Partial { path, .. } => vec![self.repository.root.join(path)],
            Operation::File { paths, .. } => paths
                .iter()
                .map(|path| self.repository.root.join(path))
                .collect(),
            Operation::Stash | Operation::Apply { .. } | Operation::Commit(_) => {
                vec![self.repository.root.clone()]
            }
        }
    }

    pub fn label(&self) -> &'static str {
        match &self.operation {
            Operation::Partial { kind, .. } => match kind {
                FileOperation::Stage => "Stage selected changes",
                FileOperation::Unstage => "Unstage selected changes",
            },
            Operation::File { kind, .. } => kind.label(),
            Operation::Stash => "Stash all repository changes",
            Operation::Apply { .. } => "Apply stash (keep stash)",
            Operation::Commit(_) => "Commit staged changes",
        }
    }

    /// Once started, a mutation must be driven to completion, even after its originating UI closes.
    pub async fn execute(self) -> Result<()> {
        self.execute_with_notice().await.map(|_| ())
    }

    /// A committed change remains successful even if newer staging prevents refreshing the index.
    pub async fn execute_with_notice(self) -> Result<Option<String>> {
        let prepared = tokio::task::spawn_blocking(move || {
            ensure!(
                self.expected
                    == self
                        .repository
                        .stamp(self.expected.iter().map(|(path, _)| path.clone())),
                "Git state changed while preparing this action; refresh and try again"
            );
            Ok::<_, anyhow::Error>(self)
        })
        .await??;
        let mut command = prepared.repository.command();
        match &prepared.operation {
            Operation::Partial { patch, .. } => {
                use tokio::io::AsyncWriteExt;
                command
                    .args(["apply", "--cached", "--whitespace=nowarn", "--recount", "-"])
                    .stdin(Stdio::piped());
                let mut child = command.spawn().context("Could not start Git")?;
                let mut input = child.stdin.take().unwrap();
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();
                let (write, status, out, err) = tokio::join!(
                    async {
                        input.write_all(patch).await?;
                        input.shutdown().await?;
                        drop(input);
                        Ok::<_, std::io::Error>(())
                    },
                    child.wait(),
                    output(stdout),
                    output(stderr)
                );
                let status = status?;
                let _ = out?;
                let (err, _) = err?;
                ensure!(
                    status.success(),
                    "Git could not apply selected changes: {}",
                    literal(&err)
                );
                write?;
                return Ok(());
            }
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
            Operation::Commit(files) => {
                command
                    .args(["commit", "--cleanup=strip", "--file"])
                    .arg(dunce::simplified(files.path()))
                    .env("GIT_INDEX_FILE", dunce::simplified(files.index_path()));
                let result = run(command).await;
                let notice = tokio::task::spawn_blocking(move || {
                    let Operation::Commit(files) = &prepared.operation else {
                        unreachable!()
                    };
                    files.reconcile(&prepared.repository)
                })
                .await
                .unwrap_or_else(|error| {
                    log::warn!("Commit index refresh worker failed: {error}");
                    Some("Staging area could not be refreshed; review staged changes".into())
                });
                return match result {
                    Ok(_) => Ok(notice),
                    Err(error) => match notice {
                        Some(notice) => Err(anyhow::anyhow!("{error}; {notice}")),
                        None => Err(error),
                    },
                };
            }
        }
        run(command).await.map(|_| None)
    }
}

impl GitRepository {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    pub fn discover(path: &Path) -> Result<Self> {
        let directory = path
            .ancestors()
            .find(|path| path.is_dir())
            .context("Git scope no longer exists")?;
        let repo = crate::git::open_repo_for_operations(directory, false)?.to_thread_local();
        let root = stdx::path::canonicalize_existing(
            repo.workdir()
                .context("Git repository has no working tree")?,
        );
        Ok(Self {
            root,
            git_dir: stdx::path::canonicalize_existing(repo.git_dir()),
            common_dir: stdx::path::canonicalize_existing(repo.common_dir()),
        })
    }

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

        let repo = self.open(true)?;
        match kind {
            FileOperation::Stage => ensure!(
                status.unstaged.is_some(),
                "This file has no unstaged changes"
            ),
            FileOperation::Unstage => {
                ensure!(status.staged.is_some(), "This file has no staged changes")
            }
        }
        let mut paths = Vec::new();
        let changes: Vec<_> = match kind {
            FileOperation::Stage => status.unstaged.iter().collect(),
            FileOperation::Unstage => status.staged.iter().collect(),
        };
        for change in changes {
            if let FileChange::Renamed { from_path, .. } = change {
                paths.push(self.relative(from_path)?);
            }
            paths.push(self.relative(change.path())?);
        }
        paths.sort();
        paths.dedup();
        ensure!(!paths.is_empty(), "No file changes selected");
        let index = repo.index_or_empty()?;
        for path in &paths {
            let bytes = gix::path::try_into_bstr(path)?;
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
        let mut watched = crate::git::repository_watched_paths(&repo)?;
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
        let expected = self.stamp(crate::git::repository_watched_paths(&repo)?);
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
        let expected = self.stamp(crate::git::repository_watched_paths(&repo)?);
        Ok(PreparedOperation {
            repository: self.clone(),
            operation: Operation::Apply {
                id: entry.id.clone(),
                restore_index,
            },
            expected,
        })
    }

    pub(super) fn open(&self, trusted: bool) -> Result<gix::Repository> {
        let repo = crate::git::open_repo_for_operations(&self.root, trusted)?.to_thread_local();
        ensure!(
            stdx::path::canonicalize_existing(repo.git_dir()) == self.git_dir
                && stdx::path::canonicalize_existing(repo.common_dir()) == self.common_dir
                && repo
                    .workdir()
                    .is_some_and(|root| stdx::path::canonicalize_existing(root) == self.root),
            "Git repository changed; reopen the picker"
        );
        Ok(repo)
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
        let mut command = Command::new("git");
        // Git for Windows rejects Rust's verbatim paths (\\?\C:\...). Keep native
        // canonical paths for identity checks, but simplify them at the CLI boundary.
        command
            .current_dir(dunce::simplified(&self.root))
            .arg("--no-pager")
            .arg("--git-dir")
            .arg(dunce::simplified(&self.git_dir))
            .arg("--work-tree")
            .arg(dunce::simplified(&self.root))
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "log.showSignature=false",
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // git rev-parse --local-env-vars: never inherit an index/worktree from the launching shell.
        for name in [
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_OBJECT_DIRECTORY",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_IMPLICIT_WORK_TREE",
            "GIT_GRAFT_FILE",
            "GIT_INDEX_FILE",
            "GIT_NO_REPLACE_OBJECTS",
            "GIT_REPLACE_REF_BASE",
            "GIT_PREFIX",
            "GIT_SHALLOW_FILE",
            "GIT_COMMON_DIR",
        ] {
            command.env_remove(name);
        }
        // Remove pathspec modes too: literal filenames must remain literal under inherited options.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_CONFIG_KEY_")
                || name.to_string_lossy().starts_with("GIT_CONFIG_VALUE_")
            {
                command.env_remove(name);
            }
        }
        for name in [
            "GIT_GLOB_PATHSPECS",
            "GIT_NOGLOB_PATHSPECS",
            "GIT_ICASE_PATHSPECS",
            "GIT_LITERAL_PATHSPECS",
        ] {
            command.env_remove(name);
        }
        command
    }
}

// Use literal magic on each user path. The global --literal-pathspecs option also reaches
// Git's internal stash/clean subprocesses and can prevent untracked-file cleanup.
fn literal_pathspec(path: &PathBuf) -> std::ffi::OsString {
    let mut argument = std::ffi::OsString::from(":(literal)");
    argument.push(path);
    argument
}

/// Bounded retained output, while continuing to drain both pipes so Git never blocks on them.
async fn output(mut reader: impl AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
    const LIMIT: usize = 1024 * 1024;
    let mut result = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let retained = read.min(LIMIT - result.len());
        result.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((result, truncated))
}

async fn run(mut command: Command) -> Result<Vec<u8>> {
    let mut child = command
        .spawn()
        .context("Could not start Git; install git and ensure it is on PATH")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (status, stdout, stderr) = tokio::join!(child.wait(), output(stdout), output(stderr));
    let status = status?;
    let (stdout, out_truncated) = stdout?;
    let (stderr, err_truncated) = stderr?;
    if !status.success() {
        let message = if stderr.is_empty() { &stdout } else { &stderr };
        let message: String = String::from_utf8_lossy(message)
            .chars()
            .take(2000)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        bail!(
            "Git failed ({status}): {}{}",
            message.trim(),
            if out_truncated || err_truncated {
                " (output truncated)"
            } else {
                ""
            }
        );
    }
    ensure!(!out_truncated, "Git output exceeds the 1 MiB display limit");
    Ok(stdout)
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
