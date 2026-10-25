//! Bounded repository snapshots. Reading a comparison never writes the index or worktree.

#[cfg(feature = "git")]
use anyhow::Result;

#[cfg(feature = "git")]
use crate::FileStatus;

pub use text_diff::MAX_REVIEW_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparisonKind {
    Staged,
    Unstaged,
    /// Read only HEAD, for comparisons against an editor snapshot.
    Head,
    /// Read only the index, for comparisons against an editor snapshot.
    Index,
}

/// Both sides use worktree encoding and line-ending conversions.
#[derive(Debug)]
pub struct Comparison {
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[cfg(feature = "git")]
pub(crate) fn read_git_comparison(
    status: &FileStatus,
    kind: ComparisonKind,
    trust_full: bool,
) -> Result<Comparison> {
    use anyhow::{bail, Context};
    use gix::{bstr::ByteSlice, objs::tree::EntryKind};
    use std::path::Path;

    use crate::{git::open_repo, FileChange};

    let path = status.path();
    let directory = path
        .parent()
        .context("file has no parent")?
        .ancestors()
        .find(|path| path.is_dir())
        .context("file has no existing ancestor")?;
    let repo = open_repo(directory, trust_full)?.to_thread_local();
    let root = repo.workdir().context("repository has no worktree")?;
    let relative = |path: &Path| -> Result<_> {
        Ok(gix::path::try_into_bstr(path.strip_prefix(root)?)?.into_owned())
    };
    let read_object = |id, mode, path: &Path| read_blob(&repo, id, mode, path.strip_prefix(root)?);
    let read_head = |path: &Path| -> Result<Vec<u8>> {
        if repo.head()?.is_unborn() {
            return Ok(Vec::new());
        }
        let tree = repo.head_commit()?.tree()?;
        let Some(entry) = tree.lookup_entry_by_path(path.strip_prefix(root)?)? else {
            return Ok(Vec::new());
        };
        read_object(entry.object_id(), entry.mode().kind(), path)
    };
    if kind != ComparisonKind::Head && matches!(status.unstaged, Some(FileChange::Conflict { .. }))
    {
        bail!("Unmerged file: resolve conflicts in the working file before reviewing against the index");
    }
    let read_index = |path: &Path| -> Result<Vec<u8>> {
        let index = repo.index_or_empty()?;
        let path_bytes = relative(path)?;
        let Some(entry) = index
            .entry_by_path_and_stage(path_bytes.as_bstr(), gix::index::entry::Stage::Unconflicted)
        else {
            return Ok(Vec::new());
        };
        if entry
            .flags
            .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        {
            return Ok(Vec::new());
        }
        let mode = if entry.mode == gix::index::entry::Mode::SYMLINK {
            EntryKind::Link
        } else if entry.mode == gix::index::entry::Mode::COMMIT {
            EntryKind::Commit
        } else {
            EntryKind::Blob
        };
        read_object(entry.id, mode, path)
    };
    fn source(change: &FileChange) -> &Path {
        match change {
            FileChange::Renamed { from_path, .. } => from_path,
            _ => change.path(),
        }
    }
    let (before, after) = match kind {
        ComparisonKind::Head => {
            let original = status
                .staged
                .as_ref()
                .or(status.unstaged.as_ref())
                .map_or(path, source);
            (read_head(original)?, Vec::new())
        }
        ComparisonKind::Index => {
            let original = status.unstaged.as_ref().map_or(path, source);
            (read_index(original)?, Vec::new())
        }
        ComparisonKind::Staged => {
            let path = status.staged.as_ref().map_or(path, FileChange::path);
            let original = status.staged.as_ref().map_or(path, source);
            (read_head(original)?, read_index(path)?)
        }
        ComparisonKind::Unstaged => {
            let original = status.unstaged.as_ref().map_or(path, source);
            let before = read_index(original)?;
            let after = match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    gix::path::try_into_bstr(std::fs::read_link(path)?)?
                        .into_owned()
                        .into()
                }
                Ok(metadata) if metadata.is_file() => {
                    bounded_read(&mut std::fs::File::open(path)?)?
                }
                Ok(_) => bail!("Submodule changes cannot be displayed as a text diff"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.into()),
            };
            (before, after)
        }
    };
    Ok(Comparison { before, after })
}

/// Read a bounded historical blob, applying only the repository's permitted worktree filters.
#[cfg(feature = "git")]
pub(crate) fn read_blob(
    repo: &gix::Repository,
    id: gix::ObjectId,
    mode: gix::objs::tree::EntryKind,
    path: &std::path::Path,
) -> Result<Vec<u8>> {
    use gix::objs::tree::EntryKind;
    anyhow::ensure!(
        matches!(
            mode,
            EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link
        ),
        "Submodule changes cannot be displayed as a text diff"
    );
    anyhow::ensure!(
        repo.find_header(id)?.size() <= MAX_REVIEW_BYTES as u64,
        "File too large to review (limit 10 MiB)"
    );
    let object = repo.find_object(id)?;
    if mode == EntryKind::Link {
        return Ok(object.data.to_vec());
    }
    let (mut pipeline, _) = repo.filter_pipeline(None)?;
    let relative = gix::path::try_into_bstr(path)?;
    let mut outcome = pipeline.convert_to_worktree(
        &object.data,
        relative.as_ref(),
        gix::filter::plumbing::driver::apply::Delay::Forbid,
    )?;
    bounded_read(&mut outcome)
}

#[cfg(feature = "git")]
fn bounded_read(reader: &mut impl std::io::Read) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader
        .take(MAX_REVIEW_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_REVIEW_BYTES,
        "File too large to review (limit 10 MiB)"
    );
    Ok(bytes)
}

#[cfg(all(test, feature = "git"))]
mod tests {
    use super::*;
    use crate::{
        git,
        git::test::{create_commit_with_message, empty_git_repo as git_repo, exec_git_cmd},
        ChangedFileScope, FileChange,
    };
    use std::cell::RefCell;

    fn empty_git_repo() -> tempfile::TempDir {
        let repo = git_repo();
        std::fs::write(repo.path().join(".gitattributes"), "* text eol=lf\n").unwrap();
        repo
    }

    fn statuses(root: &std::path::Path) -> Vec<FileStatus> {
        let results = RefCell::new(Vec::new());
        git::for_each_file_status(
            &ChangedFileScope::Repository(root.into()),
            false,
            |_, result| {
                results.borrow_mut().push(result.unwrap());
                true
            },
        )
        .unwrap();
        results.into_inner()
    }

    #[test]
    fn partially_staged_files_keep_both_changes_even_when_worktree_matches_head() {
        let repo = empty_git_repo();
        let path = repo.path().join("file.txt");
        std::fs::write(&path, "head\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        std::fs::write(&path, "index\n").unwrap();
        exec_git_cmd(&["add", "file.txt"], repo.path());
        std::fs::write(&path, "head\n").unwrap();
        let index = std::fs::read(repo.path().join(".git/index")).unwrap();
        let statuses = statuses(repo.path());
        assert_eq!(statuses.len(), 1);
        let status = &statuses[0];
        assert!(matches!(status.staged, Some(FileChange::Modified { .. })));
        assert!(matches!(status.unstaged, Some(FileChange::Modified { .. })));
        let staged = read_git_comparison(status, ComparisonKind::Staged, false).unwrap();
        assert_eq!(
            (staged.before.as_slice(), staged.after.as_slice()),
            (b"head\n".as_slice(), b"index\n".as_slice())
        );
        let unstaged = read_git_comparison(status, ComparisonKind::Unstaged, false).unwrap();
        assert_eq!(
            (unstaged.before.as_slice(), unstaged.after.as_slice()),
            (b"index\n".as_slice(), b"head\n".as_slice())
        );
        assert_eq!(
            std::fs::read(repo.path().join(".git/index")).unwrap(),
            index
        );
    }

    #[test]
    fn additions_before_first_commit_and_intent_to_add_have_empty_bases() {
        let repo = empty_git_repo();
        let path = repo.path().join("file.txt");
        std::fs::write(&path, "new\n").unwrap();
        exec_git_cmd(&["add", "-N", "file.txt"], repo.path());
        let status = statuses(repo.path()).pop().unwrap();
        let comparison = read_git_comparison(&status, ComparisonKind::Unstaged, false).unwrap();
        assert!(comparison.before.is_empty());
        assert_eq!(comparison.after, b"new\n");
        exec_git_cmd(&["add", "file.txt"], repo.path());
        let status = statuses(repo.path()).pop().unwrap();
        let comparison = read_git_comparison(&status, ComparisonKind::Staged, false).unwrap();
        assert!(comparison.before.is_empty());
        assert_eq!(comparison.after, b"new\n");
    }

    #[test]
    fn deleted_files_can_be_reviewed_after_their_parent_directory_is_removed() {
        let repo = empty_git_repo();
        let folder = repo.path().join("nested");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("file.txt"), "deleted\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        std::fs::remove_dir_all(&folder).unwrap();
        let status = statuses(repo.path()).pop().unwrap();
        let comparison = read_git_comparison(&status, ComparisonKind::Unstaged, false).unwrap();
        assert_eq!(comparison.before, b"deleted\n");
        assert!(comparison.after.is_empty());
        exec_git_cmd(&["add", "-u"], repo.path());
        let status = statuses(repo.path()).pop().unwrap();
        let comparison = read_git_comparison(&status, ComparisonKind::Staged, false).unwrap();
        assert_eq!(comparison.before, b"deleted\n");
        assert!(comparison.after.is_empty());
    }

    #[test]
    fn staged_rename_and_following_edit_use_the_correct_paths_and_crlf_filters() {
        let repo = empty_git_repo();
        std::fs::write(repo.path().join(".gitattributes"), "*.txt text eol=crlf\n").unwrap();
        std::fs::write(repo.path().join("old.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        exec_git_cmd(&["mv", "old.txt", "new.txt"], repo.path());
        std::fs::write(repo.path().join("new.txt"), "one\r\nedited\r\nthree\r\n").unwrap();
        let status = statuses(repo.path()).pop().unwrap();
        assert!(matches!(status.staged, Some(FileChange::Renamed { .. })));
        assert!(matches!(status.unstaged, Some(FileChange::Modified { .. })));
        let staged = read_git_comparison(&status, ComparisonKind::Staged, false).unwrap();
        assert_eq!(staged.before, staged.after);
        assert_eq!(staged.before, b"one\r\ntwo\r\nthree\r\n");
        let unstaged = read_git_comparison(&status, ComparisonKind::Unstaged, false).unwrap();
        assert_eq!(unstaged.before, staged.after);
        assert_eq!(unstaged.after, b"one\r\nedited\r\nthree\r\n");
        let head = read_git_comparison(&status, ComparisonKind::Head, false).unwrap();
        assert_eq!(head.before, staged.before);
    }

    #[test]
    fn successive_staged_and_unstaged_renames_share_one_row_and_keep_each_base() {
        let repo = empty_git_repo();
        std::fs::write(repo.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        exec_git_cmd(&["mv", "a.txt", "b.txt"], repo.path());
        std::fs::rename(repo.path().join("b.txt"), repo.path().join("c.txt")).unwrap();
        let statuses = statuses(repo.path());
        assert_eq!(statuses.len(), 1, "{statuses:?}");
        let status = &statuses[0];
        assert_eq!(status.path(), repo.path().join("c.txt"));
        assert!(matches!(status.staged, Some(FileChange::Renamed { .. })));
        assert!(matches!(status.unstaged, Some(FileChange::Renamed { .. })));
        for kind in [ComparisonKind::Staged, ComparisonKind::Unstaged] {
            let comparison = read_git_comparison(status, kind, false).unwrap();
            assert_eq!(comparison.before, b"one\ntwo\nthree\n");
            assert_eq!(comparison.before, comparison.after);
        }
        let head = read_git_comparison(status, ComparisonKind::Head, false).unwrap();
        assert_eq!(head.before, b"one\ntwo\nthree\n");
    }

    #[test]
    fn linked_worktree_comparisons_read_the_worktree_index() {
        let repo = empty_git_repo();
        std::fs::write(repo.path().join("file.txt"), "head\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        let linked = tempfile::tempdir().unwrap();
        let linked = linked.path().join("checkout");
        exec_git_cmd(
            &["worktree", "add", "-b", "review", linked.to_str().unwrap()],
            repo.path(),
        );
        std::fs::write(linked.join("file.txt"), "linked index\n").unwrap();
        exec_git_cmd(&["add", "file.txt"], &linked);
        let status = statuses(&linked).pop().unwrap();
        let comparison = read_git_comparison(&status, ComparisonKind::Staged, false).unwrap();
        assert_eq!(comparison.before, b"head\n");
        assert_eq!(comparison.after, b"linked index\n");
        assert!(statuses(repo.path()).is_empty());
    }

    #[test]
    fn oversized_worktree_and_index_contents_are_rejected() {
        let repo = empty_git_repo();
        std::fs::write(
            repo.path().join("large.txt"),
            vec![b'x'; MAX_REVIEW_BYTES + 1],
        )
        .unwrap();
        let status = statuses(repo.path()).pop().unwrap();
        assert!(
            read_git_comparison(&status, ComparisonKind::Unstaged, false)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
        exec_git_cmd(&["add", "large.txt"], repo.path());
        let status = statuses(repo.path()).pop().unwrap();
        assert!(read_git_comparison(&status, ComparisonKind::Staged, false)
            .unwrap_err()
            .to_string()
            .contains("too large"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_review_reads_the_link_without_following_its_target() {
        let repo = empty_git_repo();
        std::os::unix::fs::symlink("outside-before", repo.path().join("link")).unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        std::fs::remove_file(repo.path().join("link")).unwrap();
        std::os::unix::fs::symlink("outside-after", repo.path().join("link")).unwrap();
        let status = statuses(repo.path()).pop().unwrap();
        let comparison = read_git_comparison(&status, ComparisonKind::Unstaged, false).unwrap();
        assert_eq!(comparison.before, b"outside-before");
        assert_eq!(comparison.after, b"outside-after");
    }
}

#[cfg(all(test, feature = "git"))]
mod trust_tests {
    use super::*;
    use crate::{
        git::test::{create_commit_with_message, empty_git_repo, exec_git_cmd},
        FileChange,
    };

    #[test]
    fn reduced_trust_does_not_execute_repository_local_smudge_filters() {
        let repo = empty_git_repo();
        let path = repo.path().join("file.txt");
        // Exercise streaming beyond pipe buffers: the filter must read its entire input.
        let original = b"original\n".repeat(16 * 1024);
        std::fs::write(&path, &original).unwrap();
        std::fs::write(
            repo.path().join(".gitattributes"),
            "*.txt text eol=lf filter=probe\n",
        )
        .unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        exec_git_cmd(
            &["config", "filter.probe.smudge", "git hash-object --stdin"],
            repo.path(),
        );
        let status = FileStatus {
            staged: Some(FileChange::Modified { path }),
            unstaged: None,
        };
        let reduced = read_git_comparison(&status, ComparisonKind::Staged, false).unwrap();
        assert_eq!(reduced.before, original);
        assert_eq!(reduced.after, original);
        assert_eq!(
            crate::git::get_diff_base(status.path(), false).unwrap(),
            original
        );
        let trusted = read_git_comparison(&status, ComparisonKind::Staged, true).unwrap();
        let expected =
            gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &original)
                .unwrap()
                .to_string();
        assert_eq!(
            String::from_utf8(trusted.before).unwrap().trim_end(),
            expected
        );
        assert_eq!(
            String::from_utf8(trusted.after).unwrap().trim_end(),
            expected
        );
    }

    #[test]
    fn git_safety_allowlist_does_not_override_workspace_trust() {
        const CHILD: &str = "MITOS_TEST_REVIEW_TRUST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            reduced_trust_does_not_execute_repository_local_smudge_filters();
            return;
        }

        // Isolate Git configuration without changing the environment of parallel tests.
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("gitconfig");
        std::fs::write(&config, "[safe]\n\tdirectory = *\n").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "review::trust_tests::git_safety_allowlist_does_not_override_workspace_trust",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("GIT_CONFIG_GLOBAL", &config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
