use anyhow::{bail, Context, Result};
use arc_swap::ArcSwap;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gix::bstr::{BString, ByteSlice, ByteVec};
use gix::diff::{index::Change as IndexChange, Rewrites};
use gix::dir::entry::Status;
use gix::objs::tree::EntryKind;
use gix::status::{
    index_worktree::Item,
    plumbing::index_as_worktree::{Change, EntryStatus},
    tree_index::TrackRenames,
    Item as StatusItem, UntrackedFiles,
};
use gix::{Commit, ObjectId, Repository};

use crate::repository::open_repo;
use crate::{ChangedFileScope, DocumentVcs, FileChange, FileStatus};

#[cfg(test)]
pub(super) mod test;

#[inline]
pub(super) fn get_repo_dir(file: &Path) -> Result<&Path> {
    file.parent().context("file has no parent directory")
}

pub fn inspect_file(file: &Path, trust_full: bool, read_diff: bool) -> Result<DocumentVcs> {
    debug_assert!(!file.exists() || file.is_file());
    debug_assert!(file.is_absolute());
    let file = gix::path::realpath(file).context("resolve symlinks")?;
    let repo = open_repo(get_repo_dir(&file)?, trust_full)?.to_thread_local();
    Ok(DocumentVcs {
        diff_base: read_diff
            .then(|| {
                diff_base(&repo, &file)
                    .inspect_err(|error| log::debug!("Diff base for {}: {error:#}", file.display()))
                    .ok()
            })
            .flatten(),
        head_name: head_name(&repo)
            .inspect_err(|error| log::debug!("Head name for {}: {error:#}", file.display()))
            .ok(),
    })
}

fn diff_base(repo: &Repository, file: &Path) -> Result<Vec<u8>> {
    let head = repo.head_commit()?;
    let file_oid = find_file_in_commit(repo, &head, file)?;

    let file_object = repo.find_object(file_oid)?;
    let data = file_object.detach().data;
    // Get the actual data that git would make out of the git object.
    // This will apply the user's git config or attributes like crlf conversions.
    //
    // The whole filter pipeline still runs in untrusted (`Trust::Reduced`) mode so built-in
    // conversions like autocrlf keep working, but `open_repo` excludes repository-local
    // `filter.*.clean` / `filter.*.smudge` drivers from sensitive configuration lookups, so those
    // external programs are not executed unless the workspace was explicitly trusted.
    if let Some(work_dir) = repo.workdir() {
        let rela_path = file.strip_prefix(work_dir)?;
        crate::blob::to_worktree(repo, &data, rela_path, None)
    } else {
        Ok(data)
    }
}

fn head_name(repo: &Repository) -> Result<Arc<ArcSwap<Box<str>>>> {
    let head_ref = repo.head_ref()?;
    let head_commit = repo.head_commit()?;

    let name = match head_ref {
        Some(reference) => reference.name().shorten().to_string(),
        None => head_commit.id.to_hex_with_len(8).to_string(),
    };

    Ok(Arc::new(ArcSwap::from_pointee(name.into_boxed_str())))
}

pub fn for_each_file_status(
    scope: &ChangedFileScope,
    trust_full: bool,
    f: impl Fn(&Path, Result<FileStatus>) -> bool,
) -> Result<()> {
    status(
        &open_repo(scope.path(), trust_full)?.to_thread_local(),
        scope,
        f,
    )
}

/// Emulates the result of running `git status` from the command line.
fn status(
    repo: &Repository,
    scope: &ChangedFileScope,
    f: impl Fn(&Path, Result<FileStatus>) -> bool,
) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
        .to_path_buf();

    let rewrites = Rewrites {
        copies: None,
        percentage: Some(0.5),
        limit: 1000,
        ..Default::default()
    };
    let status_platform = repo
        .status(gix::progress::Discard)?
        // Here we discard the `status.showUntrackedFiles` config, as it makes little sense in
        // our case to not list new (untracked) files. We could have respected this config
        // if the default value weren't `Collapsed` though, as this default value would render
        // the feature unusable to many.
        .untracked_files(UntrackedFiles::Files)
        // Detect renames in both the staging area and the working tree.
        .index_worktree_rewrites(Some(rewrites))
        .tree_index_track_renames(TrackRenames::Given(rewrites));

    let patterns = status_patterns(&work_dir, scope)?;

    let status_iter = status_platform.into_iter(patterns)?;
    // Keep both comparisons: collapsing them loses partially staged changes.
    let mut changes: BTreeMap<PathBuf, FileStatus> = BTreeMap::new();

    for item in status_iter {
        let item = match item {
            Ok(item) => item,
            Err(err) => {
                if !f(&work_dir, Err(err.into())) {
                    return Ok(());
                }
                continue;
            }
        };
        let staged = matches!(&item, StatusItem::TreeIndex(_));
        let change = match item {
            StatusItem::IndexWorktree(Item::Modification {
                rela_path, status, ..
            }) => {
                let path = work_dir.join(rela_path.to_path()?);
                match status {
                    EntryStatus::Conflict { .. } => FileChange::Conflict { path },
                    EntryStatus::Change(Change::Removed) => FileChange::Deleted { path },
                    EntryStatus::Change(
                        Change::Modification { .. }
                        | Change::Type { .. }
                        | Change::SubmoduleModification(_),
                    ) => FileChange::Modified { path },
                    // Files marked with `git add --intent-to-add`. Such files
                    // still show up as new in `git status`, so it's appropriate
                    // to show them the same way as untracked files in the
                    // "changed file" picker. One example of this being used
                    // is Jujutsu, a Git-compatible VCS. It marks all new files
                    // with `--intent-to-add` automatically.
                    EntryStatus::IntentToAdd => FileChange::Untracked { path },
                    _ => continue,
                }
            }
            StatusItem::IndexWorktree(Item::DirectoryContents { entry, .. })
                if entry.status == Status::Untracked =>
            {
                FileChange::Untracked {
                    path: work_dir.join(entry.rela_path.to_path()?),
                }
            }
            StatusItem::IndexWorktree(Item::Rewrite {
                source,
                dirwalk_entry,
                ..
            }) => FileChange::Renamed {
                from_path: work_dir.join(source.rela_path().to_path()?),
                to_path: work_dir.join(dirwalk_entry.rela_path.to_path()?),
            },
            StatusItem::TreeIndex(IndexChange::Addition { location, .. }) => FileChange::Added {
                path: work_dir.join(location.to_path()?),
            },
            StatusItem::TreeIndex(IndexChange::Deletion { location, .. }) => FileChange::Deleted {
                path: work_dir.join(location.to_path()?),
            },
            StatusItem::TreeIndex(IndexChange::Modification { location, .. }) => {
                FileChange::Modified {
                    path: work_dir.join(location.to_path()?),
                }
            }
            StatusItem::TreeIndex(IndexChange::Rewrite {
                source_location,
                location,
                ..
            }) => FileChange::Renamed {
                from_path: work_dir.join(source_location.to_path()?),
                to_path: work_dir.join(location.to_path()?),
            },
            _ => continue,
        };
        let entry = changes
            .entry(change.path().to_path_buf())
            .or_insert_with(|| FileStatus {
                staged: None,
                unstaged: None,
            });
        if staged {
            entry.staged = Some(change);
        } else {
            entry.unstaged = Some(change);
        }
    }

    // Join a working-tree rename to its staged source instead of showing two rows.
    let renames: Vec<_> = changes
        .iter()
        .filter_map(|(path, status)| match &status.unstaged {
            Some(FileChange::Renamed { from_path, .. }) => Some((path.clone(), from_path.clone())),
            _ => None,
        })
        .collect();
    for (destination, source) in renames {
        if changes
            .get(&destination)
            .is_some_and(|status| status.staged.is_none())
            && changes
                .get(&source)
                .is_some_and(|status| status.unstaged.is_none())
            && let Some(source_status) = changes.remove(&source)
        {
            changes.get_mut(&destination).unwrap().staged = source_status.staged;
        }
    }
    for status in changes.into_values() {
        if !f(&work_dir, Ok(status)) {
            break;
        }
    }

    Ok(())
}

fn status_patterns(work_dir: &Path, scope: &ChangedFileScope) -> Result<Vec<BString>> {
    match scope {
        ChangedFileScope::Directory(directory) => {
            let directory = gix::path::realpath(directory).context("resolve change scope")?;
            let canonical_work_dir =
                gix::path::realpath(work_dir).context("resolve repository worktree")?;
            let relative = directory
                .strip_prefix(canonical_work_dir)
                .context("change scope is outside the repository worktree")?;
            if relative.as_os_str().is_empty() {
                Ok(Vec::new())
            } else {
                let relative = gix::path::into_bstr(relative)?;
                let relative = gix::path::to_unix_separators_on_windows(relative);
                // gix otherwise resolves this repository-relative path from the process CWD,
                // which may already be inside `directory` and would prefix the scope twice.
                let mut pattern = BString::from(":(top,literal)");
                pattern.push_str(relative.as_ref().as_bytes());
                Ok(vec![pattern])
            }
        }
        ChangedFileScope::Repository(_) => Ok(Vec::new()),
    }
}

/// Finds the object that contains the contents of a file at a specific commit.
fn find_file_in_commit(repo: &Repository, commit: &Commit, file: &Path) -> Result<ObjectId> {
    let repo_dir = repo.workdir().context("repo has no worktree")?;
    let rel_path = file.strip_prefix(repo_dir)?;
    let tree = commit.tree()?;
    let tree_entry = tree
        .lookup_entry_by_path(rel_path)?
        .context("file is untracked")?;
    match tree_entry.mode().kind() {
        // not a file, everything is new, do not show diff
        mode @ (EntryKind::Tree | EntryKind::Commit | EntryKind::Link) => {
            bail!("entry at {} is not a file but a {mode:?}", file.display())
        }
        // found a file
        EntryKind::Blob | EntryKind::BlobExecutable => Ok(tree_entry.object_id()),
    }
}
