use std::{cell::RefCell, fs::File, io::Write, path::Path, process::Command};

use tempfile::TempDir;

use super::EntryKind;
use crate::{git, ChangedFileScope, DiffProviderRegistry, FileChange};

pub(crate) fn exec_git_cmd(args: &[&str], git_dir: &Path) {
    let res = Command::new("git")
        .arg("-C")
        .arg(git_dir) // execute the git command in this directory
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_TERMINAL_PROMPT", "false")
        .env("GIT_AUTHOR_DATE", "2000-01-01 00:00:00 +0000")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_NAME", "author")
        .env("GIT_COMMITTER_DATE", "2000-01-02 00:00:00 +0000")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_NAME", "committer")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap_or_else(|_| panic!("`git {args:?}` failed"));
    if !res.status.success() {
        println!("{}", String::from_utf8_lossy(&res.stdout));
        eprintln!("{}", String::from_utf8_lossy(&res.stderr));
        panic!("`git {args:?}` failed (see output above)")
    }
}

fn create_commit(repo: &Path, add_modified: bool) {
    create_commit_with_message(repo, add_modified, "message");
}

pub(crate) fn create_commit_with_message(repo: &Path, add_modified: bool, message: &str) {
    if add_modified {
        exec_git_cmd(&["add", "-A"], repo);
    }
    exec_git_cmd(&["commit", "-m", message], repo);
}

pub(crate) fn empty_git_repo() -> TempDir {
    // Fixtures use the same path spelling as editor documents on Windows.
    let tmp = tempfile::tempdir_in(stdx::path::normalize(tempfile::env::temp_dir()))
        .expect("create temp dir for git testing");
    exec_git_cmd(&["init"], tmp.path());
    exec_git_cmd(&["config", "user.email", "test@mitos.org"], tmp.path());
    exec_git_cmd(&["config", "user.name", "mitos-test"], tmp.path());
    tmp
}

#[test]
fn missing_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    File::create(&file).unwrap().write_all(b"foo").unwrap();

    assert!(diff_base(&file, true).is_err());
}

#[test]
fn unmodified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    assert_eq!(diff_base(&file, true).unwrap(), Vec::from(contents));
}

#[test]
fn modified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    File::create(&file).unwrap().write_all(b"bar").unwrap();

    assert_eq!(diff_base(&file, true).unwrap(), Vec::from(contents));
}

#[test]
fn changed_files_include_the_worktree_root_when_started_in_a_subdirectory() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    File::create(&file).unwrap().write_all(b"before").unwrap();
    create_commit(temp_git.path(), true);
    File::create(&file).unwrap().write_all(b"after").unwrap();

    let subdirectory = temp_git.path().join("nested");
    std::fs::create_dir(&subdirectory).unwrap();
    let changes = RefCell::new(Vec::new());

    git::for_each_file_status(
        &ChangedFileScope::Repository(subdirectory),
        true,
        |worktree_root, change| {
            changes.borrow_mut().push((
                worktree_root.to_path_buf(),
                change.unwrap().change().clone(),
            ));
            true
        },
    )
    .unwrap();

    let changes = changes.into_inner();
    let (worktree_root, changed) = changes
        .iter()
        .find(|(_, change)| change.path() == file)
        .expect("modified root file should be reported");
    assert_eq!(worktree_root, temp_git.path());
    assert!(matches!(changed, crate::FileChange::Modified { .. }));
}

#[test]
fn changed_files_can_be_scoped_to_a_directory() {
    let temp_git = empty_git_repo();
    let project = temp_git.path().join("project");
    let source = project.join("src");
    std::fs::create_dir_all(&source).unwrap();
    let project_file = source.join("project.txt");
    let sibling_file = temp_git.path().join("sibling.txt");
    File::create(&project_file)
        .unwrap()
        .write_all(b"before")
        .unwrap();
    File::create(&sibling_file)
        .unwrap()
        .write_all(b"before")
        .unwrap();
    create_commit(temp_git.path(), true);
    File::create(&project_file)
        .unwrap()
        .write_all(b"after")
        .unwrap();
    File::create(&sibling_file)
        .unwrap()
        .write_all(b"after")
        .unwrap();

    for staged in [false, true] {
        if staged {
            exec_git_cmd(&["add", "-A"], temp_git.path());
        }
        let changes = RefCell::new(Vec::new());
        git::for_each_file_status(
            &ChangedFileScope::Directory(project.clone()),
            true,
            |_worktree_root, change| {
                changes.borrow_mut().push(change.unwrap().change().clone());
                true
            },
        )
        .unwrap();

        let changes = changes.into_inner();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path(), project_file);
    }

    let patterns = git::status_patterns(
        temp_git.path(),
        &ChangedFileScope::Directory(project.clone()),
    )
    .unwrap();
    assert_eq!(patterns, [":(top,literal)project"]);
}

#[test]
fn changed_files_include_staged_changes_without_duplicate_paths() {
    let repo = empty_git_repo();
    let root = repo.path();
    std::fs::write(root.join(".gitattributes"), "* text eol=lf\n").unwrap();
    for (name, text) in [
        ("modified.txt", "modify this\n"),
        ("deleted.txt", "delete this\n"),
        ("original.txt", "rename this\n"),
        ("mixed.txt", "original content\n"),
    ] {
        std::fs::write(root.join(name), text).unwrap();
    }
    create_commit(root, true);
    std::fs::write(root.join("modified.txt"), "staged modification\n").unwrap();
    std::fs::write(root.join("added.txt"), "staged addition\n").unwrap();
    std::fs::write(root.join("mixed.txt"), "staged content\n").unwrap();
    exec_git_cmd(&["rm", "deleted.txt"], root);
    exec_git_cmd(&["mv", "original.txt", "renamed.txt"], root);
    exec_git_cmd(&["add", "-A"], root);
    // Keep the addition and rename classifications after further working-tree edits.
    std::fs::write(root.join("added.txt"), "staged addition\nfurther edit\n").unwrap();
    std::fs::write(root.join("renamed.txt"), "rename this\nfurther edit\n").unwrap();
    // Both comparisons matter even when the working file matches HEAD again.
    std::fs::write(root.join("mixed.txt"), "original content\n").unwrap();
    std::fs::write(root.join("untracked.txt"), "untracked\n").unwrap();
    let index = std::fs::read(root.join(".git/index")).unwrap();

    let changes = RefCell::new(Vec::new());
    git::for_each_file_status(
        &ChangedFileScope::Repository(root.into()),
        true,
        |_, change| {
            changes.borrow_mut().push(change.unwrap().change().clone());
            true
        },
    )
    .unwrap();
    let changes = changes.into_inner();
    let mut paths: Vec<_> = changes
        .iter()
        .map(|change| change.path().file_name().unwrap().to_owned())
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "added.txt",
            "deleted.txt",
            "mixed.txt",
            "modified.txt",
            "renamed.txt",
            "untracked.txt"
        ]
    );
    assert!(changes.iter().any(
        |change| matches!(change, FileChange::Added { path } if path == &root.join("added.txt"))
    ));
    assert!(changes.iter().any(
        |change| matches!(change, FileChange::Deleted { path } if path == &root.join("deleted.txt"))
    ));
    assert!(changes.iter().any(|change| matches!(change, FileChange::Modified { path } if path == &root.join("modified.txt"))));
    assert!(changes.iter().any(|change| matches!(change, FileChange::Renamed { from_path, to_path } if from_path == &root.join("original.txt") && to_path == &root.join("renamed.txt"))));
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
}

#[test]
fn changed_files_include_staged_additions_before_the_first_commit() {
    let repo = empty_git_repo();
    let root = repo.path();
    for name in ["added.txt", "intent.txt", "untracked.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    exec_git_cmd(&["add", "added.txt"], root);
    exec_git_cmd(&["add", "--intent-to-add", "intent.txt"], root);

    let changes = RefCell::new(Vec::new());
    git::for_each_file_status(
        &ChangedFileScope::Repository(root.into()),
        false,
        |_, change| {
            changes.borrow_mut().push(change.unwrap().change().clone());
            true
        },
    )
    .unwrap();
    let changes = changes.into_inner();
    assert_eq!(changes.len(), 3);
    assert!(changes.iter().any(
        |change| matches!(change, FileChange::Added { path } if path == &root.join("added.txt"))
    ));
    for name in ["intent.txt", "untracked.txt"] {
        assert!(changes.iter().any(
            |change| matches!(change, FileChange::Untracked { path } if path == &root.join(name))
        ));
    }
}

#[test]
fn changed_files_keep_conflicts_alongside_staged_changes() {
    let repo = empty_git_repo();
    let root = repo.path();
    std::fs::write(root.join("conflict.txt"), "base\n").unwrap();
    std::fs::write(root.join("staged.txt"), "before\n").unwrap();
    create_commit(root, true);
    exec_git_cmd(&["checkout", "-b", "other"], root);
    std::fs::write(root.join("conflict.txt"), "theirs\n").unwrap();
    create_commit(root, true);
    exec_git_cmd(&["checkout", "main"], root);
    std::fs::write(root.join("conflict.txt"), "ours\n").unwrap();
    create_commit(root, true);
    let merge = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge", "--no-commit", "other"])
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1), "merge should conflict");
    std::fs::write(root.join("staged.txt"), "after\n").unwrap();
    exec_git_cmd(&["add", "staged.txt"], root);

    let changes = RefCell::new(Vec::new());
    git::for_each_file_status(
        &ChangedFileScope::Repository(root.into()),
        true,
        |_, change| {
            changes.borrow_mut().push(change.unwrap().change().clone());
            true
        },
    )
    .unwrap();
    let changes = changes.into_inner();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|change| matches!(change, FileChange::Conflict { path } if path == &root.join("conflict.txt"))));
    assert!(changes.iter().any(
        |change| matches!(change, FileChange::Modified { path } if path == &root.join("staged.txt"))
    ));
}

#[test]
fn colocated_jj_uses_the_first_conflict_side_for_staged_changes() {
    let repo = empty_git_repo();
    let root = repo.path();
    std::fs::create_dir(root.join(".jj")).unwrap();
    for (rel, content) in [
        ("conflict.txt", "side 0\n"),
        ("real-old.txt", "renamed\n"),
        ("real_deleted.txt", "deleted\n"),
        ("sub/JJ-CONFLICT-README", "nested readme\n"),
        ("sub/.jjconflict-base-0/nested.txt", "nested\n"),
    ] {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    create_commit(root, true);

    let git_repo = gix::open(root).unwrap();
    let head = git_repo.head_commit().unwrap();
    let first_side = head.tree().unwrap();
    let base_blob = git_repo.write_blob(b"base\n").unwrap();
    let second_blob = git_repo.write_blob(b"side 1\n").unwrap();
    let added_blob = git_repo
        .write_blob(b"unique content from conflict side 1\n")
        .unwrap();
    let readme = git_repo
        .write_blob(b"This commit was made by jj\n")
        .unwrap();

    let mut side = first_side.edit().unwrap();
    side.upsert("conflict.txt", EntryKind::Blob, base_blob)
        .unwrap();
    let base = side.write().unwrap();
    side.upsert("conflict.txt", EntryKind::Blob, second_blob)
        .unwrap();
    side.upsert("modify_delete.txt", EntryKind::Blob, added_blob)
        .unwrap();
    let second_side = side.write().unwrap();

    // jj adds conflict metadata to Git's HEAD tree but leaves it out of the index.
    let mut tree = first_side.edit().unwrap();
    tree.upsert(".jjconflict-base-0", EntryKind::Tree, base)
        .unwrap();
    tree.upsert(".jjconflict-side-0", EntryKind::Tree, first_side.id)
        .unwrap();
    tree.upsert(".jjconflict-side-1", EntryKind::Tree, second_side)
        .unwrap();
    tree.upsert("JJ-CONFLICT-README", EntryKind::Blob, readme)
        .unwrap();
    let mut conflict: gix::objs::Commit = head.decode().unwrap().try_into().unwrap();
    conflict.tree = tree.write().unwrap().detach();
    conflict.extra_headers.push((
        "jj:trees".into(),
        format!("{} {base} {second_side}", first_side.id).into(),
    ));
    let conflict = git_repo.write_object(conflict).unwrap();
    exec_git_cmd(
        &["update-ref", "--no-deref", "HEAD", &conflict.to_string()],
        root,
    );

    exec_git_cmd(&["mv", "real-old.txt", "real-new.txt"], root);
    // Real deletions (including similarly named nested files) must remain visible.
    exec_git_cmd(
        &[
            "rm",
            "real_deleted.txt",
            "sub/JJ-CONFLICT-README",
            "sub/.jjconflict-base-0/nested.txt",
        ],
        root,
    );
    std::fs::write(
        root.join("conflict.txt"),
        "<<<<<<< Conflict 1 of 1\nside 0\n=======\nside 1\n>>>>>>>\n",
    )
    .unwrap();
    // An added file matching a conflict side must not be reported as renamed from .jjconflict-*.
    std::fs::write(
        root.join("modify_delete.txt"),
        "unique content from conflict side 1\n",
    )
    .unwrap();
    exec_git_cmd(&["add", "modify_delete.txt"], root);

    let statuses = RefCell::new(Vec::new());
    git::for_each_file_status(
        &ChangedFileScope::Repository(root.into()),
        true,
        |_, status| {
            statuses.borrow_mut().push(status.unwrap());
            true
        },
    )
    .unwrap();
    let statuses = statuses.into_inner();
    let mut paths: Vec<_> = statuses
        .iter()
        .map(|status| status.path().strip_prefix(root).unwrap().to_path_buf())
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            std::path::PathBuf::from("conflict.txt"),
            std::path::PathBuf::from("modify_delete.txt"),
            std::path::PathBuf::from("real-new.txt"),
            std::path::PathBuf::from("real_deleted.txt"),
            std::path::PathBuf::from("sub/.jjconflict-base-0/nested.txt"),
            std::path::PathBuf::from("sub/JJ-CONFLICT-README"),
        ]
    );
    assert!(statuses.iter().any(|status| matches!(
        &status.staged,
        Some(FileChange::Renamed { from_path, to_path })
            if from_path == &root.join("real-old.txt") && to_path == &root.join("real-new.txt")
    )));
    assert!(statuses.iter().any(|status| matches!(
        status,
        crate::FileStatus {
            staged: None,
            unstaged: Some(FileChange::Modified { path }),
        } if path == &root.join("conflict.txt")
    )));
    assert!(statuses.iter().any(|status| matches!(
        status,
        crate::FileStatus {
            staged: Some(FileChange::Added { path }),
            unstaged: None,
        } if path == &root.join("modify_delete.txt")
    )));
}

#[test]
fn conflict_free_repos_keep_jj_named_staged_deletions() {
    let repo = empty_git_repo();
    let root = repo.path();
    for (rel, content) in [
        ("JJ-CONFLICT-README", "readme\n"),
        (".jjconflict-base-0/file.txt", "base\n"),
        (".jjconflict-side-0/file.txt", "side\n"),
    ] {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    create_commit(root, true);
    exec_git_cmd(
        &[
            "rm",
            "-r",
            "JJ-CONFLICT-README",
            ".jjconflict-base-0",
            ".jjconflict-side-0",
        ],
        root,
    );

    for colocated in [false, true] {
        if colocated {
            std::fs::create_dir(root.join(".jj")).unwrap();
        }
        let changes = RefCell::new(Vec::new());
        git::for_each_file_status(
            &ChangedFileScope::Repository(root.into()),
            true,
            |_, status| {
                changes.borrow_mut().push(status.unwrap().change().clone());
                true
            },
        )
        .unwrap();
        let changes = changes.into_inner();
        assert_eq!(changes.len(), 3);
        assert!(changes
            .iter()
            .all(|change| matches!(change, FileChange::Deleted { .. })));
    }
}

/// Test that `get_file_head` does not return content for a directory.
/// This is important to correctly cover cases where a directory is removed and replaced by a file.
/// If the contents of the directory object were returned a diff between a path and the directory children would be produced.
#[test]
fn directory() {
    let temp_git = empty_git_repo();
    let dir = temp_git.path().join("file.txt");
    std::fs::create_dir(&dir).expect("");
    let file = dir.join("file.txt");
    let contents = b"foo".as_slice();
    File::create(file).unwrap().write_all(contents).unwrap();

    create_commit(temp_git.path(), true);

    std::fs::remove_dir_all(&dir).unwrap();
    File::create(&dir).unwrap().write_all(b"bar").unwrap();
    assert!(diff_base(&dir, true).is_err());
}

/// Test that document inspection resolves symlinks so that the same diff base is
/// used as the target file.
///
/// This is important to correctly cover cases where a symlink is removed and
/// replaced by a file. If the contents of the symlink object were returned
/// a diff between a literal file path and the actual file content would be
/// produced (bad ui).
#[cfg(any(unix, windows))]
#[test]
fn symlink() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    let file_link = temp_git.path().join("file_link.txt");

    symlink("file.txt", &file_link).unwrap();
    create_commit(temp_git.path(), true);

    assert_eq!(diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(diff_base(&file, true).unwrap(), contents);
}

/// Test that document inspection returns content when the file is a symlink to
/// another file that is in a git repo, but the symlink itself is not.
#[cfg(any(unix, windows))]
#[test]
fn symlink_to_git_repo() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let temp_git = empty_git_repo();

    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    create_commit(temp_git.path(), true);

    let file_link = temp_dir.path().join("file_link.txt");
    symlink(&file, &file_link).unwrap();

    assert_eq!(diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(diff_base(&file, true).unwrap(), contents);
}

#[test]
fn ignored_revision_paths_use_the_git_installation_prefix() {
    let repo = empty_git_repo();
    exec_git_cmd(
        &[
            "config",
            "blame.ignoreRevsFile",
            "%(prefix)/ignored-revisions",
        ],
        repo.path(),
    );
    let output = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["config", "--path", "--get", "blame.ignoreRevsFile"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let expected = Path::new(std::str::from_utf8(&output.stdout).unwrap().trim_end());
    let paths = watched_files(repo.path(), false).unwrap();
    assert!(paths.contains(&expected.to_path_buf()));
}

#[test]
fn watched_paths_include_ignored_revision_files_before_creation() {
    let repo = empty_git_repo();
    std::fs::write(repo.path().join("file.txt"), "line\n").unwrap();
    create_commit_with_message(repo.path(), true, "initial");
    exec_git_cmd(&["tag", "ignored"], repo.path());
    exec_git_cmd(&["config", "blame.ignoreRevs", "ignored"], repo.path());
    let conventional = repo.path().join(".git-blame-ignore-revs");
    let paths = watched_paths(repo.path(), false).unwrap();
    assert!(paths.contains(&repo.path().join(".git/refs/tags/ignored")));
    assert!(!paths.contains(&conventional));
    assert!(watched_files(repo.path(), false)
        .unwrap()
        .contains(&conventional));
    let external = tempfile::tempdir().unwrap();
    let configured = external.path().join("ignored revisions");
    exec_git_cmd(
        &[
            "config",
            "blame.ignoreRevsFile",
            configured.to_str().unwrap(),
        ],
        repo.path(),
    );
    let paths = watched_files(repo.path(), false).unwrap();
    assert!(paths.contains(&configured));
    assert!(!paths.contains(&conventional));
    exec_git_cmd(
        &["config", "--add", "blame.ignoreRevsFile", ""],
        repo.path(),
    );
    let paths = watched_files(repo.path(), false).unwrap();
    assert!(!paths.contains(&configured));
    assert!(!paths.contains(&conventional));
}

#[test]
fn watched_paths_include_branch_refs_and_linked_worktree_metadata() {
    let repo = empty_git_repo();
    std::fs::write(repo.path().join("file.txt"), "hello\n").unwrap();
    create_commit(repo.path(), true);
    let paths = watched_paths(repo.path(), false).unwrap();
    assert!(paths.iter().any(|path| path.ends_with(".git/index")));
    assert!(paths.iter().any(|path| path.ends_with(".git/HEAD")));
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/refs/heads/main")));
    assert!(paths.iter().any(|path| path.ends_with(".git/packed-refs")));
    assert!(paths.iter().any(|path| path.ends_with(".git/config")));
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/config.worktree")));
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/info/attributes")));
    exec_git_cmd(&["worktree", "add", "-b", "feature", "linked"], repo.path());
    let paths = watched_paths(&repo.path().join("linked"), false).unwrap();
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/worktrees/linked/index")));
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/worktrees/linked/HEAD")));
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/refs/heads/feature")));
    assert!(paths.iter().any(|path| path.ends_with(".git/packed-refs")));
    assert!(paths.iter().any(|path| path.ends_with(".git/config")));
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/worktrees/linked/config.worktree")));
    assert!(paths
        .iter()
        .any(|path| path.ends_with(".git/info/attributes")));
}

// Exercise the public combined inspections rather than reopening in separate test paths.
fn diff_base(file: &Path, trust_full: bool) -> anyhow::Result<Vec<u8>> {
    git::inspect_file(file, trust_full, true)?
        .diff_base
        .ok_or_else(|| anyhow::anyhow!("no diff base"))
}

fn watched_paths(path: &Path, trust_full: bool) -> anyhow::Result<Vec<std::path::PathBuf>> {
    Ok(crate::repository::repository_watches(path, trust_full)?.paths)
}

fn watched_files(path: &Path, trust_full: bool) -> anyhow::Result<Vec<std::path::PathBuf>> {
    Ok(crate::repository::repository_watches(path, trust_full)?.files)
}

#[test]
fn inspection_keeps_head_for_untracked_files_and_refreshes_linked_worktrees() {
    let repo = empty_git_repo();
    exec_git_cmd(&["config", "core.autocrlf", "false"], repo.path());
    let tracked = repo.path().join("file.txt");
    std::fs::write(&tracked, "main\n").unwrap();
    create_commit(repo.path(), true);
    exec_git_cmd(&["worktree", "add", "-b", "feature", "linked"], repo.path());
    let linked = repo.path().join("linked");
    std::fs::write(linked.join("file.txt"), "feature\n").unwrap();
    create_commit(&linked, true);
    // Git's linked-worktree backpointer may spell the worktree through an alias.
    #[cfg(unix)]
    let aliases = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        let alias = aliases.path().join("linked");
        std::os::unix::fs::symlink(&linked, &alias).unwrap();
        std::fs::write(
            repo.path().join(".git/worktrees/linked/gitdir"),
            format!("{}\n", alias.join(".git").display()),
        )
        .unwrap();
    }
    let providers = crate::DiffProviderRegistry::default();
    for (root, branch, contents) in [
        (repo.path(), "main", b"main\n".as_slice()),
        (linked.as_path(), "feature", b"feature\n".as_slice()),
    ] {
        let inspected = providers.inspect_file(&root.join("file.txt"), false, true);
        assert_eq!(
            inspected.diff_base.as_deref(),
            Some(contents),
            "diff lookup: {:?}",
            crate::repository::open_repo(root, false).and_then(|repo| {
                let file = gix::path::realpath(root.join("file.txt"))?;
                git::diff_base(&repo.to_thread_local(), &file)
            })
        );
        assert_eq!(&**inspected.head_name.unwrap().load_full(), branch);
        let inspected = providers.inspect_file(&root.join("untracked.txt"), false, true);
        assert!(inspected.diff_base.is_none());
        assert_eq!(&**inspected.head_name.unwrap().load_full(), branch);
    }
    exec_git_cmd(&["checkout", "--detach"], &linked);
    let inspected = providers.inspect_file(&linked.join("file.txt"), false, false);
    assert!(inspected.diff_base.is_none());
    assert_eq!(inspected.head_name.unwrap().load().len(), 8);
    let main = providers.inspect_file(&tracked, false, false);
    assert_eq!(&**main.head_name.unwrap().load_full(), "main");
}

#[test]
fn watched_paths_include_missing_and_existing_external_attributes() {
    let repo = empty_git_repo();
    let external = tempfile::tempdir().unwrap();
    let attributes = external.path().join("global attributes");
    exec_git_cmd(
        &[
            "config",
            "core.attributesFile",
            attributes.to_str().unwrap(),
        ],
        repo.path(),
    );
    // Reduced trust ignores this repository-local sensitive configuration.
    assert!(!watched_paths(repo.path(), false)
        .unwrap()
        .contains(&attributes));
    assert!(watched_paths(repo.path(), true)
        .unwrap()
        .contains(&attributes));
    std::fs::write(&attributes, "*.txt text eol=crlf\n").unwrap();
    assert!(watched_paths(repo.path(), true)
        .unwrap()
        .contains(&attributes));
}

#[cfg(unix)]
#[test]
fn global_attribute_targets_resolve_symlinked_parents_before_relative_components() {
    let repo = empty_git_repo();
    let external = tempfile::tempdir().unwrap();
    let root = external.path().canonicalize().unwrap();
    let real_directory = root.join("store/config/git");
    std::fs::create_dir_all(&real_directory).unwrap();
    std::os::unix::fs::symlink(root.join("store/config"), root.join("config")).unwrap();
    std::os::unix::fs::symlink(
        "../../../source-attributes",
        real_directory.join("attributes"),
    )
    .unwrap();
    let configured = root.join("config/git/attributes");
    let target = root.join("source-attributes");
    exec_git_cmd(
        &[
            "config",
            "core.attributesFile",
            configured.to_str().unwrap(),
        ],
        repo.path(),
    );
    for exists in [false, true] {
        if exists {
            std::fs::write(&target, "*.txt -text\n").unwrap();
        }
        let paths = watched_paths(repo.path(), true).unwrap();
        assert!(paths.contains(&configured));
        assert!(paths.contains(&target));
        // Lexically normalizing the configured spelling would escape the fixture.
        assert!(!paths.contains(&root.parent().unwrap().join("source-attributes")));
        assert!(!paths.contains(&std::path::PathBuf::from("/")));
    }
}

#[tokio::test]
async fn changed_files_preserve_repository_discovery_errors() {
    let directory = tempfile::tempdir().unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    DiffProviderRegistry::default().for_each_file_status(
        ChangedFileScope::Directory(directory.path().to_owned()),
        false,
        move |_, result| {
            sender.send(result).unwrap();
            false
        },
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(format!("{:#}", result.unwrap_err()).contains("failed to discover git repo"));
}

#[test]
fn diff_base_accepts_a_relative_worktree_path() {
    let repository = empty_git_repo();
    exec_git_cmd(&["config", "core.autocrlf", "false"], repository.path());
    std::fs::write(repository.path().join("file.txt"), "base\n").unwrap();
    create_commit(repository.path(), true);
    exec_git_cmd(
        &["worktree", "add", "-b", "relative", "linked"],
        repository.path(),
    );
    // Relative backlinks should not require a recent Git CLI to construct the fixture.
    std::fs::write(
        repository.path().join("linked/.git"),
        "gitdir: ../.git/worktrees/linked\n",
    )
    .unwrap();
    std::fs::write(
        repository.path().join(".git/worktrees/linked/gitdir"),
        "../../../linked/.git\n",
    )
    .unwrap();
    let file = repository.path().join("linked/file.txt");
    for trust_full in [false, true] {
        // Windows canonical paths have a verbatim prefix unlike Git's metadata.
        for path in [file.clone(), file.canonicalize().unwrap()] {
            assert_eq!(
                git::inspect_file(&path, trust_full, true)
                    .unwrap()
                    .diff_base
                    .unwrap(),
                b"base\n"
            );
        }
    }
    std::fs::write(&file, "changed\n").unwrap();
    let directory = file.parent().unwrap();
    for trust_full in [false, true] {
        for scope in [
            ChangedFileScope::Repository(directory.to_owned()),
            ChangedFileScope::Directory(directory.to_owned()),
            ChangedFileScope::Directory(directory.canonicalize().unwrap()),
        ] {
            let expected_root = stdx::path::normalize(scope.path());
            let found = std::cell::Cell::new(false);
            git::for_each_file_status(&scope, trust_full, |root, change| {
                assert_eq!(root, expected_root);
                if change.unwrap().path() == expected_root.join("file.txt") {
                    found.set(true);
                }
                true
            })
            .unwrap();
            assert!(found.get());
        }
    }
}

#[tokio::test]
async fn file_status_cancellation_does_not_report_a_provider_failure() {
    let repository = empty_git_repo();
    std::fs::write(repository.path().join("first.txt"), "first\n").unwrap();
    std::fs::write(repository.path().join("second.txt"), "second\n").unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    DiffProviderRegistry::default().for_each_file_status(
        ChangedFileScope::Repository(repository.path().to_owned()),
        false,
        move |_, result| {
            sender.send(result).unwrap();
            false
        },
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        receiver.recv().await.unwrap().unwrap();
        assert!(receiver.recv().await.is_none());
    })
    .await
    .unwrap();
}

#[test]
fn directory_scopes_accept_native_and_normalized_path_spellings() {
    let directory = tempfile::tempdir().unwrap();
    let root = stdx::path::normalize(directory.path());
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    for path in [
        directory.path().to_owned(),
        directory.path().canonicalize().unwrap(),
    ] {
        let patterns = git::status_patterns(&root, &ChangedFileScope::Directory(path)).unwrap();
        assert!(patterns.is_empty());
    }
    for path in [nested.clone(), nested.canonicalize().unwrap()] {
        let patterns = git::status_patterns(&root, &ChangedFileScope::Directory(path)).unwrap();
        assert_eq!(patterns, [":(top,literal)nested"]);
    }
}
