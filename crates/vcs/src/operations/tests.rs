use super::*;
use crate::git::test::{create_commit_with_message, empty_git_repo, exec_git_cmd};

fn statuses(root: &Path) -> Vec<FileStatus> {
    let result = std::cell::RefCell::new(Vec::new());
    crate::git::for_each_file_status(
        &crate::ChangedFileScope::Repository(root.into()),
        false,
        |_, status| {
            result.borrow_mut().push(status.unwrap());
            true
        },
    )
    .unwrap();
    result.into_inner()
}

fn initial() -> tempfile::TempDir {
    let repo = empty_git_repo();
    std::fs::write(repo.path().join(".gitattributes"), "* text eol=lf\n").unwrap();
    std::fs::write(repo.path().join("file.txt"), "head\n").unwrap();
    std::fs::write(repo.path().join("other.txt"), "other\n").unwrap();
    create_commit_with_message(repo.path(), true, "initial");
    repo
}

#[tokio::test]
async fn stage_and_unstage_leave_working_file_untouched() -> Result<()> {
    let repo = initial();
    let path = repo.path().join("file.txt");
    let git = GitRepository::discover(repo.path())?;
    std::fs::write(&path, "index\n")?;
    git.prepare_file(&statuses(repo.path())[0], FileOperation::Stage, true)?
        .execute()
        .await?;
    std::fs::write(&path, "worktree\n")?;
    let status = statuses(repo.path()).remove(0);
    assert!(status.staged.is_some() && status.unstaged.is_some());
    git.prepare_file(&status, FileOperation::Unstage, true)?
        .execute()
        .await?;
    assert!(statuses(repo.path())[0].staged.is_none());
    assert_eq!(std::fs::read_to_string(path)?, "worktree\n");
    Ok(())
}

#[tokio::test]
async fn unborn_head_and_literal_pathspecs() -> Result<()> {
    let repo = empty_git_repo();
    let git = GitRepository::discover(repo.path())?;
    let name = "[literal].txt";
    std::fs::write(repo.path().join(name), "literal")?;
    std::fs::write(repo.path().join("l.txt"), "other")?;
    let status = statuses(repo.path())
        .into_iter()
        .find(|status| status.path().ends_with(name))
        .unwrap();
    git.prepare_file(&status, FileOperation::Stage, true)?
        .execute()
        .await?;
    let status = statuses(repo.path())
        .into_iter()
        .find(|status| status.path().ends_with(name))
        .unwrap();
    assert!(status.staged.is_some());
    assert!(statuses(repo.path())
        .iter()
        .find(|status| status.path().ends_with("l.txt"))
        .unwrap()
        .staged
        .is_none());
    git.prepare_file(&status, FileOperation::Unstage, true)?
        .execute()
        .await?;
    assert!(repo.path().join(name).exists());
    assert!(statuses(repo.path())
        .iter()
        .all(|status| status.staged.is_none()));
    Ok(())
}

#[tokio::test]
async fn stash_all_records_both_layers_and_apply_keeps_entry() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    std::fs::write(repo.path().join("file.txt"), "index\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    std::fs::write(repo.path().join("file.txt"), "worktree\n")?;
    std::fs::write(repo.path().join("other.txt"), "unrelated\n")?;
    exec_git_cmd(&["add", "other.txt"], repo.path());
    git.prepare_stash(true)?.execute().await?;
    assert_eq!(
        std::fs::read_to_string(repo.path().join("file.txt"))?,
        "head\n"
    );
    let remaining = statuses(repo.path());
    assert!(remaining.is_empty());
    let entries = git.stashes().await?;
    assert_eq!(entries.len(), 1);
    let files = git.stash_files(&entries[0]).await?;
    assert_eq!(files.len(), 4);
    let comparison = git.read_stash_file(
        &entries[0],
        files
            .iter()
            .find(|file| file.path().ends_with("file.txt") && file.layer() == StashLayer::Worktree)
            .unwrap(),
        false,
    )?;
    assert_eq!(comparison.before, b"head\n");
    assert_eq!(comparison.after, b"worktree\n");
    git.prepare_apply(&entries[0], false, true)?
        .execute()
        .await?;
    assert_eq!(
        std::fs::read_to_string(repo.path().join("file.txt"))?,
        "worktree\n"
    );
    assert_eq!(git.stashes().await?, entries);
    Ok(())
}

#[tokio::test]
async fn untracked_stash_is_inspectable_and_restored() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    std::fs::write(repo.path().join("new.txt"), "new\n")?;
    git.prepare_stash(true)?.execute().await?;
    assert!(!repo.path().join("new.txt").exists());
    let entry = git.stashes().await?.remove(0);
    let files = git.stash_files(&entry).await?;
    assert_eq!(files.len(), 1);
    assert!(files[0].untracked());
    assert_eq!(
        git.read_stash_file(&entry, &files[0], false)?.after,
        b"new\n"
    );
    git.prepare_apply(&entry, true, true)?.execute().await?;
    assert_eq!(
        std::fs::read_to_string(repo.path().join("new.txt"))?,
        "new\n"
    );
    Ok(())
}

#[tokio::test]
async fn stale_or_untrusted_plan_does_not_write() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    let path = repo.path().join("file.txt");
    std::fs::write(&path, "new")?;
    let status = statuses(repo.path()).remove(0);
    assert!(git
        .prepare_file(&status, FileOperation::Stage, false)
        .is_err());
    let plan = git.prepare_file(&status, FileOperation::Stage, true)?;
    std::fs::write(path, "newer content")?;
    assert!(plan
        .execute()
        .await
        .unwrap_err()
        .to_string()
        .contains("Git state changed"));
    assert!(statuses(repo.path())[0].staged.is_none());
    Ok(())
}

#[tokio::test]
async fn apply_uses_captured_oid_and_restores_the_saved_index() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    let path = repo.path().join("file.txt");
    std::fs::write(&path, "staged\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    std::fs::write(&path, "worktree\n")?;
    git.prepare_stash(true)?.execute().await?;
    let original = git.stashes().await?.remove(0);
    std::fs::write(&path, "newer stash\n")?;
    git.prepare_stash(true)?.execute().await?;
    git.prepare_apply(&original, true, true)?.execute().await?;
    assert_eq!(std::fs::read_to_string(&path)?, "worktree\n");
    let status = statuses(repo.path()).remove(0);
    let staged = crate::review::read_git_comparison(&status, crate::ComparisonKind::Staged, false)?;
    assert_eq!(staged.after, b"staged\n");
    assert_eq!(git.stashes().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn conflicted_application_retains_the_stash_and_reports_the_failure() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    let path = repo.path().join("file.txt");
    std::fs::write(&path, "stashed\n")?;
    git.prepare_stash(true)?.execute().await?;
    let entry = git.stashes().await?.remove(0);
    std::fs::write(&path, "committed after stash\n")?;
    create_commit_with_message(repo.path(), true, "later");
    assert!(git
        .prepare_apply(&entry, false, true)?
        .execute()
        .await
        .is_err());
    assert!(std::fs::read_to_string(path)?.contains("<<<<<<<"));
    assert_eq!(git.stashes().await?, vec![entry]);
    assert!(git.prepare_stash(true).is_err());
    Ok(())
}

#[tokio::test]
async fn rename_unstage_restores_both_index_paths_and_stage_records_deletion() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    exec_git_cmd(&["mv", "file.txt", "renamed.txt"], repo.path());
    let status = statuses(repo.path()).remove(0);
    assert!(matches!(status.staged, Some(FileChange::Renamed { .. })));
    git.prepare_file(&status, FileOperation::Unstage, true)?
        .execute()
        .await?;
    assert!(statuses(repo.path())
        .iter()
        .all(|status| status.staged.is_none()));
    let renamed = statuses(repo.path()).remove(0);
    git.prepare_file(&renamed, FileOperation::Stage, true)?
        .execute()
        .await?;
    assert!(statuses(repo.path())
        .iter()
        .any(|status| matches!(status.staged, Some(FileChange::Renamed { .. }))));
    std::fs::remove_file(repo.path().join("renamed.txt"))?;
    let deleted = statuses(repo.path()).remove(0);
    git.prepare_file(&deleted, FileOperation::Stage, true)?
        .execute()
        .await?;
    assert!(statuses(repo.path())
        .iter()
        .any(|status| matches!(status.staged, Some(FileChange::Deleted { .. }))));
    Ok(())
}

#[tokio::test]
async fn linked_worktree_actions_leave_the_main_worktree_index_untouched() -> Result<()> {
    let repo = initial();
    let linked = tempfile::tempdir()?;
    exec_git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            "linked",
            linked.path().to_str().unwrap(),
        ],
        repo.path(),
    );
    let main_index = std::fs::read(repo.path().join(".git/index"))?;
    let git = GitRepository::discover(linked.path())?;
    std::fs::write(linked.path().join("file.txt"), "linked\n")?;
    git.prepare_file(&statuses(linked.path())[0], FileOperation::Stage, true)?
        .execute()
        .await?;
    assert_eq!(std::fs::read(repo.path().join(".git/index"))?, main_index);
    git.prepare_stash(true)?.execute().await?;
    let entry = git.stashes().await?.remove(0);
    git.prepare_apply(&entry, true, true)?.execute().await?;
    assert_eq!(
        std::fs::read_to_string(linked.path().join("file.txt"))?,
        "linked\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("file.txt"))?,
        "head\n"
    );
    assert_eq!(std::fs::read(repo.path().join(".git/index"))?, main_index);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_to_directories_are_staged_without_following_the_leaf() -> Result<()> {
    use std::os::unix::fs::symlink;
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    symlink(".", repo.path().join("directory-link"))?;
    for status in statuses(repo.path()) {
        git.prepare_file(&status, FileOperation::Stage, true)?
            .execute()
            .await?;
    }
    assert!(statuses(repo.path())
        .iter()
        .all(|status| status.staged.is_some() && status.unstaged.is_none()));
    Ok(())
}

#[tokio::test]
async fn inherited_environment_cannot_redirect_writes() -> Result<()> {
    const ROOT: &str = "MITOS_TEST_GIT_ACTION_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = PathBuf::from(root);
        let git = GitRepository::discover(&root)?;
        assert_eq!(git.root(), stdx::path::canonicalize_existing(&root));
        let status = FileStatus {
            staged: None,
            unstaged: Some(FileChange::Modified {
                path: root.join("file.txt"),
            }),
        };
        git.prepare_file(&status, FileOperation::Stage, true)?
            .execute()
            .await?;
        return Ok(());
    }
    let repo = initial();
    let other = initial();
    std::fs::write(repo.path().join("file.txt"), "selected\n")?;
    let other_index = std::fs::read(other.path().join(".git/index"))?;
    // A separate test process avoids changing the environment of concurrently running tests.
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "operations::tests::inherited_environment_cannot_redirect_writes",
            "--nocapture",
        ])
        .env(ROOT, repo.path())
        .env("GIT_DIR", other.path().join(".git"))
        .env("GIT_WORK_TREE", other.path())
        .env("GIT_INDEX_FILE", other.path().join(".git/index"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", other.path())
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_GLOB_PATHSPECS", "1")
        .output()?;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(other.path().join(".git/index"))?, other_index);
    assert_eq!(
        std::fs::read_to_string(other.path().join("file.txt"))?,
        "head\n"
    );
    assert!(statuses(repo.path())[0].staged.is_some());
    Ok(())
}

#[tokio::test]
async fn an_existing_index_lock_is_preserved_and_never_overwritten() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    std::fs::write(repo.path().join("file.txt"), "edited\n")?;
    let prepared = git.prepare_file(&statuses(repo.path())[0], FileOperation::Stage, true)?;
    let index = std::fs::read(repo.path().join(".git/index"))?;
    std::fs::write(repo.path().join(".git/index.lock"), "another process")?;
    assert!(prepared.execute().await.is_err());
    assert_eq!(std::fs::read(repo.path().join(".git/index"))?, index);
    assert_eq!(
        std::fs::read_to_string(repo.path().join(".git/index.lock"))?,
        "another process"
    );
    Ok(())
}

#[tokio::test]
async fn stash_inspection_includes_index_only_changes_when_worktree_matches_head() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    std::fs::write(repo.path().join("file.txt"), "staged\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    std::fs::write(repo.path().join("file.txt"), "head\n")?;
    git.prepare_stash(true)?.execute().await?;
    let entry = git.stashes().await?.remove(0);
    let files = git.stash_files(&entry).await?;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].layer(), StashLayer::Staged);
    let comparison = git.read_stash_file(&entry, &files[0], false)?;
    assert_eq!(comparison.before, b"head\n");
    assert_eq!(comparison.after, b"staged\n");
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
#[tokio::test]
async fn non_utf8_filenames_remain_literal() -> Result<()> {
    use std::os::unix::ffi::OsStringExt;
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    let name = std::ffi::OsString::from_vec(b"literal-\xff".to_vec());
    std::fs::write(repo.path().join(&name), "bytes\n")?;
    git.prepare_file(&statuses(repo.path())[0], FileOperation::Stage, true)?
        .execute()
        .await?;
    assert!(statuses(repo.path())[0].staged.is_some());
    Ok(())
}
