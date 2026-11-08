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
            Self::CreateBranch { .. } => unreachable!("branch creation runs in the native worker"),
            Self::SwitchBranch { name } => {
                command.args(["switch", "--no-guess", "--no-overwrite-ignore", "--", name]);
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
                let capture = repo.clone();
                let selected = remote.to_owned();
                if tokio::task::spawn_blocking(move || fetch::execute(&capture, &selected, None))
                    .await??
                    .is_some()
                {
                    return Ok(());
                }
                command.args(["fetch", "--no-recurse-submodules", "--", remote]);
            }
            Self::Pull { remote } => {
                return pull(repo, remote.as_deref().context("Choose a remote first")?).await;
            }
            Self::Push { remote } => {
                let git = repo
                    .open(true)
                    .map_err(|_| anyhow::anyhow!("Could not read Git repository settings"))?;
                let (branch, destination) =
                    current_branch(&git, remote.as_deref().context("Choose a remote first")?)?;
                let remote = remote.as_deref().unwrap();
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

/// Git pull cannot disable overwriting ignored files. Fetch first, then use
/// merge's guarded checkout against an immutable fetched commit.
async fn pull(repo: &GitRepository, remote: &str) -> Result<()> {
    let capture = repo.clone();
    let selected = remote.to_owned();
    let (branch, destination, head) = tokio::task::spawn_blocking(move || {
        let git = capture
            .open(true)
            .map_err(|_| anyhow::anyhow!("Could not read Git repository settings"))?;
        let (branch, destination) = current_branch(&git, &selected)?;
        Ok::<_, anyhow::Error>((branch, destination, current_head(&git)?))
    })
    .await??;
    let capture = repo.clone();
    let selected = remote.to_owned();
    let requested = destination.clone();
    let fetched =
        tokio::task::spawn_blocking(move || fetch::execute(&capture, &selected, Some(&requested)))
            .await??;
    let target = if let Some(fetched) = fetched {
        Some(
            fetched
                .target
                .context("Remote did not provide the requested branch")?,
        )
    } else {
        let mut fetch = repo.command();
        let reference = format!("refs/heads/{destination}");
        fetch.args(["fetch", "--no-recurse-submodules", "--", remote, &reference]);
        run_private(fetch, "Fetch for pull").await?;
        None
    };

    let validate = repo.clone();
    let target = tokio::task::spawn_blocking(move || {
        let git = validate
            .open_read(true)
            .map_err(|_| anyhow::anyhow!("Could not read Git repository settings"))?;
        let name = format!("refs/heads/{branch}");
        ensure!(
            git.head_name()?
                .as_ref()
                .is_some_and(|current| current.as_bstr() == name.as_bytes().as_bstr())
                && current_head(&git)? == head
                && git.state().is_none(),
            "Git branch or HEAD changed during fetch; retry pull"
        );
        target.map_or_else(|| fetched_commit(&git), Ok)
    })
    .await??;

    let mut merge = repo.command();
    merge.args([
        "-c",
        "submodule.recurse=false",
        "-c",
        "merge.autoStash=false",
        "merge",
        "--ff-only",
        "--no-autostash",
        "--no-overwrite-ignore",
        "--",
        target.as_str(),
    ]);
    run_private(merge, "Pull (fast-forward only)")
        .await
        .map(|_| ())
}

// Resolve and peel once so checkout receives an immutable commit ID.
fn fetched_commit(repo: &gix::Repository) -> Result<history::RevisionId> {
    let id = repo
        .rev_parse_single("FETCH_HEAD^{commit}")
        .map_err(|_| anyhow::anyhow!("Could not read fetched commit; retry pull"))?;
    history::RevisionId::parse(&id.to_string())
}

fn current_head(repo: &gix::Repository) -> Result<Option<gix::ObjectId>> {
    Ok(if repo.head()?.is_unborn() {
        None
    } else {
        Some(repo.head_id()?.detach())
    })
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
        let mut cursor = Some(self.branch_cursor());
        let mut branches = Vec::new();
        while let Some(current) = cursor {
            let page = current.next_page().await?;
            branches.extend(page.branches);
            cursor = page.next;
        }
        branches.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(branches)
    }

    pub async fn worktrees(&self) -> Result<Vec<Worktree>> {
        let repository = self.clone();
        tokio::task::spawn_blocking(move || {
            let repo = repository.open_read(false)?;
            let main = repo.main_repo()?;
            let mut result = vec![Worktree {
                path: stdx::path::canonicalize_existing(
                    main.workdir().unwrap_or(main.common_dir()),
                ),
                branch: if main.is_bare() {
                    None
                } else {
                    local_head(&main)?
                },
                main: true,
                locked: false,
                prunable: false,
            }];
            let mut bytes = 0;
            for worktree in repo.worktrees()? {
                let path = stdx::path::canonicalize_existing(&worktree.base()?);
                bytes += path.as_os_str().as_encoded_bytes().len();
                ensure!(
                    bytes <= 1024 * 1024,
                    "Worktree listing exceeds the 1 MiB display limit"
                );
                let locked = worktree.is_locked();
                let prunable = !locked
                    && match std::fs::metadata(path.join(".git")) {
                        Ok(_) => false,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                        Err(error) => return Err(error.into()),
                    };
                let linked = worktree.into_repo_with_possibly_inaccessible_worktree()?;
                result.push(Worktree {
                    path,
                    branch: local_head(&linked)?,
                    main: false,
                    locked,
                    prunable,
                });
            }
            result[1..].sort_by(|a, b| a.path.cmp(&b.path));
            Ok(result)
        })
        .await?
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
        let repo = if matches!(operation, RepositoryOperation::CreateBranch { .. }) {
            self.open_read(true)
        } else {
            self.open(true)
        }
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

// Only read tracking names and refspecs. The high-level remote lookup filters
// repository-local remotes in reduced-trust workspaces, even for display.
fn upstream_name(repo: &gix::Repository, branch: &gix::refs::FullNameRef) -> Result<String> {
    let config = repo.config_snapshot();
    let config = config.plumbing();
    let short = branch.shorten();
    let Some(remote) = config.string_by("branch", Some(short), "remote") else {
        return Ok(String::new());
    };
    let Some(merge) = config.string_by("branch", Some(short), "merge") else {
        return Ok(String::new());
    };
    let merge = gix::refs::FullName::try_from(merge.into_owned())?;
    if *remote == "." {
        return Ok(merge.shorten().to_str()?.to_owned());
    }
    let Some(specs) = config.strings_by("remote", Some(remote.as_ref()), "fetch") else {
        return Ok(String::new());
    };
    let specs = specs
        .iter()
        .map(|spec| gix::refspec::parse(spec.as_ref(), gix::refspec::parse::Operation::Fetch))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let null = repo.object_hash().null();
    let mappings = gix::refspec::MatchGroup::from_fetch_specs(specs).match_lhs(
        [gix::refspec::match_group::Item {
            full_ref_name: merge.as_bstr(),
            target: &null,
            object: None,
        }]
        .into_iter(),
    );
    mappings
        .mappings
        .into_iter()
        .find_map(|mapping| mapping.rhs)
        .map(|name| {
            Ok(gix::refs::FullName::try_from(name.into_owned())?
                .shorten()
                .to_str()?
                .to_owned())
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn local_head(repo: &gix::Repository) -> Result<Option<String>> {
    repo.head_name()?
        .map(|name| {
            Ok(name
                .as_bstr()
                .to_str()?
                .strip_prefix("refs/heads/")
                .map(str::to_owned))
        })
        .transpose()
        .map(Option::flatten)
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
    let path = path.to_owned();
    let common_dir = owner.common_dir.clone();
    tokio::task::spawn_blocking(move || {
        let repository = GitRepository::discover(&path)?;
        ensure!(repository.root == path && repository.common_dir == common_dir,
            "Selected path is no longer a linked worktree of this repository");
        // Status needs Git for Windows' installed filter/line-ending defaults too.
        let repo = repository.open(false)?;
        let index = repo.index_or_empty()?;
        let hidden = gix::index::entry::Flags::ASSUME_VALID | gix::index::entry::Flags::SKIP_WORKTREE;
        ensure!(!index.entries().iter().any(|entry| entry.flags.intersects(hidden)),
            "Worktree has assume-unchanged or skip-worktree index entries; inspect these files in Git before removing it");
        let mut status = repo.status(gix::progress::Discard)?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .dirwalk_options(|options| options.emit_ignored(Some(gix::dir::walk::EmissionMode::Matching)))
            .index_worktree_submodules(gix::status::Submodule::Given {
                ignore: gix::submodule::config::Ignore::None,
                check_dirty: true,
            })
            .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
            .into_iter(std::iter::empty::<gix::bstr::BString>())?;
        if let Some(change) = status.next() {
            change?;
            bail!("Worktree contains modified, untracked, or ignored files; clean it before removal");
        }
        Ok(())
    }).await?
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

mod branches;
pub use branches::{BranchCursor, BranchPage, BRANCH_PAGE_SIZE};
