//! Captured Git repository identity and configuration/trust policies shared by reads and mutations.
use anyhow::{ensure, Context, Result};
use gix::{bstr::ByteSlice, sec::trust::DefaultForLevel, Repository, ThreadSafeRepository};
use std::path::{Path, PathBuf};

/// Paths are captured before any subprocess starts, including in linked worktrees.
#[derive(Clone, Debug)]
pub struct GitRepository {
    pub(crate) root: PathBuf,
    pub(crate) git_dir: PathBuf,
    pub(crate) common_dir: PathBuf,
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
        let repo = open_repo_for_read(directory, false)?.to_thread_local();
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

    pub(crate) fn open(&self, trusted: bool) -> Result<gix::Repository> {
        self.verify_repository(open_repo_for_operations(&self.root, trusted)?)
    }

    pub(crate) fn open_read(&self, trusted: bool) -> Result<gix::Repository> {
        self.verify_repository(open_repo_for_read(&self.root, trusted)?)
    }

    pub(crate) fn verify_repository(
        &self,
        repository: gix::ThreadSafeRepository,
    ) -> Result<gix::Repository> {
        let repo = repository.to_thread_local();
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
}

pub(super) fn open_repo(path: &Path, trust_full: bool) -> Result<ThreadSafeRepository> {
    open_repo_impl(path, trust_full, true, true)
}

/// Preflight for mutations uses the selected repository rather than a launching Git process's
/// temporary object store or environment configuration overrides.
fn open_repo_for_operations(path: &Path, trust_full: bool) -> Result<ThreadSafeRepository> {
    open_repo_impl(path, trust_full, false, true)
}

/// Native repository reads must also work without Git for Windows installed.
fn open_repo_for_read(path: &Path, trust_full: bool) -> Result<ThreadSafeRepository> {
    open_repo_impl(path, trust_full, false, false)
}

fn open_repo_impl(
    path: &Path,
    trust_full: bool,
    inherit_environment: bool,
    allow_git_binary: bool,
) -> Result<ThreadSafeRepository> {
    // Discover paths before opening so gix's discovery cannot replace the caller's workspace
    // trust decision with an ownership check. `with(trust)` sets the initial level, but gix can
    // still promote configuration metadata through `safe.directory`. Filter sensitive lookups by
    // source as well so repository-local programs stay disabled in reduced-trust workspaces.

    let trust = if trust_full {
        gix::sec::Trust::Full
    } else {
        gix::sec::Trust::Reduced
    };

    // On Windows various configuration options are bundled as part of the git installation. The
    // lookup is expensive; only do it there.
    let config = gix::open::permissions::Config {
        system: true,
        git: true,
        user: true,
        env: inherit_environment,
        includes: true,
        git_binary: cfg!(windows) && allow_git_binary,
    };

    let mut permissions = gix::open::Permissions {
        config,
        ..gix::open::Permissions::default_for_level(trust)
    };
    if !inherit_environment {
        permissions.env.objects = gix::sec::Permission::Deny;
        permissions.env.git_prefix = gix::sec::Permission::Deny;
    }

    let discover_options = gix::discover::upwards::Options {
        dot_git_only: true,
        ..Default::default()
    };
    let (repo_path, _trust_from_ownership) = gix::discover::upwards_opts(path, discover_options)
        .context("failed to discover git repo")?;
    let (git_dir, work_dir) = repo_path.into_repository_and_work_tree_directories();
    // Preserve the discovered worktree spelling instead of reconstructing it from
    // a linked worktree's gitdir backpointer, which may use a different path alias.
    let open_path = work_dir.map_or(git_dir, |root| root.join(".git"));

    let options = gix::open::Options::default()
        .permissions(permissions)
        // The path already names `.git` (a directory or linked-worktree file).
        // Open it as-is rather than letting gix append `.git` again.
        .open_path_as_is(true)
        .with(trust);
    let options = if trust_full {
        options
    } else {
        options.filter_config_section(|meta| {
            meta.source.kind() != gix::config::source::Kind::Repository
        })
    };

    Ok(ThreadSafeRepository::open_opts(open_path, options)?)
}

/// Watch comparison inputs, including configuration and linked-worktree metadata.
pub fn repository_watches(path: &Path, trust_full: bool) -> Result<crate::RepositoryWatches> {
    let repo = open_repo(path, trust_full)?.to_thread_local();
    Ok(crate::RepositoryWatches {
        paths: repository_watched_paths(&repo).unwrap_or_default(),
        files: crate::blame::watched_files(&repo).unwrap_or_default(),
    })
}

pub(super) fn repository_watched_paths(repo: &Repository) -> Result<Vec<PathBuf>> {
    let git_dir = repo.git_dir().canonicalize()?;
    let common_dir = repo.common_dir().canonicalize()?;
    let mut paths = vec![
        git_dir.join("index"),
        git_dir.join("HEAD"),
        common_dir.join("packed-refs"),
        common_dir.join("config"),
        git_dir.join("config.worktree"),
        common_dir.join("info/attributes"),
    ];
    if let Some(head) = repo.head_name()? {
        paths.push(common_dir.join(head.as_bstr().to_str()?));
    }
    // Missing global files remain watch inputs so their creation refreshes filtered content.
    // A malformed attribute setting must not disable the other repository watches.
    for path in repo.attribute_global_paths().unwrap_or_default() {
        // Stamps must see target edits as well as replacement of a configured symlink.
        // Resolve a dangling link explicitly so creation of its target invalidates a plan too.
        if let Ok(target) = gix::path::realpath(&path) {
            paths.push(target);
        }
        paths.push(path);
    }
    // Invalid blame configuration must not disable the other repository watches.
    paths.extend(crate::blame::watched_paths(repo).unwrap_or_default());
    Ok(paths)
}
