//! Branches, linked worktrees, and explicit remote operations.
//! Commands never invoke a shell and remote output is never exposed to callers.

use super::*;
use gix::bstr::ByteSlice;

#[derive(Clone, Debug)]
pub enum RepositoryOperation {
    CreateBranch {
        name: String,
    },
    SwitchBranch {
        name: String,
    },
    CreateWorktree {
        path: PathBuf,
        branch: Option<String>,
    },
    RemoveWorktree {
        path: PathBuf,
    },
    Fetch {
        remote: Option<String>,
    },
    Pull {
        remote: Option<String>,
    },
    Push {
        remote: Option<String>,
    },
}

impl RepositoryOperation {
    pub fn label(&self) -> &'static str {
        match self {
            Self::CreateBranch { .. } => "Create branch",
            Self::SwitchBranch { .. } => "Switch branch",
            Self::CreateWorktree { .. } => "Create linked worktree",
            Self::RemoveWorktree { .. } => "Remove linked worktree",
            Self::Fetch { .. } => "Fetch remote",
            Self::Pull { .. } => "Pull (fast-forward only)",
            Self::Push { .. } => "Push current branch",
        }
    }
    pub(super) fn changes_worktree(&self) -> bool {
        matches!(
            self,
            Self::SwitchBranch { .. }
                | Self::CreateWorktree { .. }
                | Self::RemoveWorktree { .. }
                | Self::Pull { .. }
        )
    }
    pub(super) fn affected_paths(&self, repo: &GitRepository) -> Vec<PathBuf> {
        match self {
            Self::CreateWorktree { path, .. } | Self::RemoveWorktree { path } => vec![path.clone()],
            Self::SwitchBranch { .. } | Self::Pull { .. } => vec![repo.root.clone()],
            _ => Vec::new(),
        }
    }

    pub(super) async fn execute(&self, repo: &GitRepository) -> Result<()> {
        let mut command = repo.command();
        match self {
            Self::CreateBranch { name } => {
                command.args(["branch", "--", name]);
            }
            Self::SwitchBranch { name } => {
                command.args(["switch", "--no-guess", "--", name]);
            }
            Self::CreateWorktree { path, branch } => {
                ensure!(
                    std::fs::symlink_metadata(path).is_err(),
                    "The new worktree path already exists"
                );
                command.args(["worktree", "add"]);
                if branch.is_none() {
                    command.arg("--detach");
                }
                command
                    .arg("--")
                    .arg(dunce::simplified(path))
                    .arg(branch.as_deref().unwrap_or("HEAD"));
            }
            Self::RemoveWorktree { path } => {
                let worktrees = repo.worktrees().await?;
                let worktree = worktrees
                    .iter()
                    .find(|tree| tree.path == *path)
                    .context("Selected worktree is no longer registered")?;
                ensure!(
                    !worktree.main && !worktree.locked && *path != repo.root,
                    "Cannot remove the main, current, or locked worktree"
                );
                ensure_clean_worktree(path, repo).await?;
                command
                    .args(["worktree", "remove", "--"])
                    .arg(dunce::simplified(path));
            }
            Self::Fetch { remote } => {
                let remote = remote.as_deref().context("Choose a remote first")?;
                command.args(["fetch", "--no-recurse-submodules", "--", remote]);
            }
            Self::Pull { remote } | Self::Push { remote } => {
                let git = repo
                    .open(true)
                    .map_err(|_| anyhow::anyhow!("Could not read Git repository settings"))?;
                let (branch, destination) =
                    current_branch(&git, remote.as_deref().context("Choose a remote first")?)?;
                let remote = remote.as_deref().unwrap();
                if matches!(self, Self::Pull { .. }) {
                    command.args([
                        "-c",
                        "merge.autoStash=false",
                        "-c",
                        "rebase.autoStash=false",
                        "pull",
                        "--ff-only",
                        "--no-rebase",
                        "--no-autostash",
                        "--no-recurse-submodules",
                        "--",
                        remote,
                        &destination,
                    ]);
                } else {
                    let refspec = format!("refs/heads/{branch}:refs/heads/{destination}");
                    // Git applies the configured mirror mode after parsing --no-mirror.
                    let mirror_config = format!("remote.{remote}.mirror=false");
                    command.args([
                        "-c",
                        &mirror_config,
                        "push",
                        "--no-force",
                        "--no-mirror",
                        "--no-follow-tags",
                        "--recurse-submodules=no",
                        "--",
                        remote,
                        &refspec,
                    ]);
                }
            }
        }
        if matches!(
            self,
            Self::Fetch { .. } | Self::Pull { .. } | Self::Push { .. }
        ) {
            run_private(command, self.label()).await.map(|_| ())
        } else {
            run(command).await.map(|_| ())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub main: bool,
    pub locked: bool,
    pub prunable: bool,
}

impl GitRepository {
    pub async fn branches(&self) -> Result<Vec<Branch>> {
        let mut command = self.command();
        command.args([
            "branch",
            "--no-color",
            "--format=%(HEAD)%00%(refname:short)%00%(upstream:short)%00",
        ]);
        let bytes = run(command).await?;
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let fields = line.split(|byte| *byte == 0).collect::<Vec<_>>();
                ensure!(fields.len() == 4, "Invalid branch listing");
                Ok(Branch {
                    name: std::str::from_utf8(fields[1])?.to_owned(),
                    current: fields[0] == b"*",
                    upstream: std::str::from_utf8(fields[2])?.to_owned(),
                })
            })
            .collect()
    }

    pub async fn worktrees(&self) -> Result<Vec<Worktree>> {
        let mut command = self.command();
        command.args(["worktree", "list", "--porcelain", "-z"]);
        let bytes = run(command).await?;
        let mut result = Vec::new();
        let mut tree: Option<Worktree> = None;
        for field in bytes.split(|byte| *byte == 0) {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                ensure!(tree.is_none(), "Invalid worktree listing");
                tree = Some(Worktree {
                    path: stdx::path::canonicalize_existing(&gix::path::from_bstr(
                        gix::bstr::BStr::new(path),
                    )),
                    branch: None,
                    main: result.is_empty(),
                    locked: false,
                    prunable: false,
                });
            } else if field.is_empty() {
                if let Some(tree) = tree.take() {
                    result.push(tree);
                }
            } else if let Some(tree) = &mut tree {
                if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
                    tree.branch = Some(std::str::from_utf8(branch)?.to_owned());
                }
                tree.locked |= field == b"locked" || field.starts_with(b"locked ");
                tree.prunable |= field == b"prunable" || field.starts_with(b"prunable ");
            }
        }
        ensure!(tree.is_none(), "Incomplete worktree listing");
        Ok(result)
    }

    /// Names only. Do not expose URLs: they can contain credentials.
    pub fn remote_names(&self) -> Result<Vec<String>> {
        let repo = self
            .open(false)
            .map_err(|_| anyhow::anyhow!("Could not read Git repository settings"))?;
        configured_remotes(&repo)
    }

    pub fn prepare_repository(
        &self,
        mut operation: RepositoryOperation,
        trusted: bool,
    ) -> Result<PreparedOperation> {
        ensure!(
            trusted,
            "Git actions require workspace trust; run :workspace-trust first"
        );
        let repo = self
            .open(true)
            .map_err(|_| anyhow::anyhow!("Could not read Git repository settings"))?;
        if !matches!(
            operation,
            RepositoryOperation::Fetch { .. } | RepositoryOperation::Push { .. }
        ) {
            ensure!(
                repo.state().is_none(),
                "Finish the current Git operation first"
            );
        }
        let mut watched = crate::git::repository_watched_paths(&repo)?;
        match &mut operation {
            RepositoryOperation::CreateBranch { name }
            | RepositoryOperation::SwitchBranch { name } => {
                validate_branch(name)?;
                watched.push(self.common_dir.join("refs/heads").join(name));
            }
            RepositoryOperation::CreateWorktree { path, branch } => {
                *path = stdx::path::canonicalize_existing(path);
                ensure!(
                    std::fs::symlink_metadata(&*path).is_err(),
                    "The new worktree path already exists"
                );
                ensure!(
                    !path.starts_with(&self.root) && !self.root.starts_with(&*path),
                    "Create linked worktrees outside the current working tree"
                );
                ensure!(
                    !path.starts_with(&self.common_dir) && !path.starts_with(&self.git_dir),
                    "Create linked worktrees outside Git metadata"
                );
                if let Some(branch) = branch {
                    validate_branch(branch)?;
                    watched.push(self.common_dir.join("refs/heads").join(branch));
                }
                watched.push(path.clone());
            }
            RepositoryOperation::RemoveWorktree { path } => {
                *path = stdx::path::canonicalize_existing(path);
                ensure!(*path != self.root, "Cannot remove the current worktree");
                watched.push(path.clone());
            }
            RepositoryOperation::Fetch { remote }
            | RepositoryOperation::Pull { remote }
            | RepositoryOperation::Push { remote } => {
                let names = configured_remotes(&repo)?;
                let selected = match remote.take() {
                    Some(name) => {
                        ensure!(names.contains(&name), "Choose a configured remote name");
                        name
                    }
                    None => {
                        ensure!(
                            names.len() == 1,
                            "Choose a remote name: {}",
                            names.join(", ")
                        );
                        names[0].clone()
                    }
                };
                *remote = Some(selected);
            }
        }
        // Linked-worktree registrations and branch refs can change outside the editor.
        watched.extend([
            self.common_dir.join("worktrees"),
            self.common_dir.join("refs/heads"),
        ]);
        Ok(PreparedOperation {
            repository: self.clone(),
            expected: self.stamp(watched),
            operation: Operation::Repository(operation),
        })
    }
}

fn validate_branch(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && !name.starts_with('-') && !name.contains("@{") && name != "HEAD",
        "Expected a branch name"
    );
    gix::refs::FullName::try_from(format!("refs/heads/{name}"))
        .map_err(|_| anyhow::anyhow!("Invalid branch name"))?;
    Ok(())
}

fn configured_remotes(repo: &gix::Repository) -> Result<Vec<String>> {
    let config = repo.config_snapshot();
    let mut names = config
        .plumbing()
        .sections_by_name("remote")
        .into_iter()
        .flatten()
        .filter_map(|section| section.header().subsection_name())
        .map(|name| {
            let name = name
                .to_str()
                .map_err(|_| anyhow::anyhow!("Remote name must be UTF-8"))?;
            ensure!(
                !name.is_empty()
                    && !name.starts_with('-')
                    && !name.chars().any(|c| c.is_control() || c.is_whitespace()),
                "Invalid remote name"
            );
            gix::refs::FullName::try_from(format!("refs/remotes/{name}/HEAD"))
                .map_err(|_| anyhow::anyhow!("Invalid remote name"))?;
            Ok(name.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    names.dedup();
    ensure!(!names.is_empty(), "No Git remotes configured");
    Ok(names)
}

fn current_branch(repo: &gix::Repository, remote: &str) -> Result<(String, String)> {
    let head = repo
        .head_name()?
        .context("Create or switch to a local branch first")?;
    let branch = head
        .as_bstr()
        .to_str()?
        .strip_prefix("refs/heads/")
        .context("Create or switch to a local branch first")?
        .to_owned();
    validate_branch(&branch)?;
    let config = repo.config_snapshot();
    let upstream_remote = config.string(&format!("branch.{branch}.remote"));
    let merge = config.string(&format!("branch.{branch}.merge"));
    let destination = if upstream_remote
        .as_deref()
        .is_some_and(|value| value == remote)
    {
        merge
            .as_ref()
            .map(|value| value.to_str())
            .transpose()?
            .and_then(|value| value.strip_prefix("refs/heads/"))
            .unwrap_or(&branch)
            .to_owned()
    } else {
        branch.clone()
    };
    validate_branch(&destination)?;
    Ok((branch, destination))
}

async fn ensure_clean_worktree(path: &Path, owner: &GitRepository) -> Result<()> {
    let repo = GitRepository::discover(path)?;
    ensure!(
        repo.root == path && repo.common_dir == owner.common_dir,
        "Selected path is no longer a linked worktree of this repository"
    );
    let mut command = repo.command();
    command.args([
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--ignored=matching",
        "-z",
    ]);
    let output = run(command).await?;
    ensure!(
        output.is_empty(),
        "Worktree contains modified, untracked, or ignored files; clean it before removal"
    );
    Ok(())
}

/// Discard error output from Git, credential helpers, and hooks. It can contain remote URLs.
async fn run_private(mut command: Command, label: &'static str) -> Result<Vec<u8>> {
    let mut child = command
        .spawn()
        .context("Could not start Git; install git and ensure it is on PATH")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (status, stdout, stderr) = tokio::join!(child.wait(), output(stdout), output(stderr));
    let status = status?;
    let (stdout, truncated) = stdout?;
    let _ = stderr?;
    ensure!(status.success(), "{label} failed ({status}). Check repository state and Git authentication in your terminal; command output is hidden because it can contain credentials");
    ensure!(!truncated, "Git listing exceeds the 1 MiB display limit");
    Ok(stdout)
}

#[cfg(test)]
mod tests;
