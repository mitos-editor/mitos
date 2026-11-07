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

fn staged_text(root: &Path, path: &str) -> String {
    let result = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!(":{path}")])
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git show :{path} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[tokio::test]
async fn partial_staging_rejects_custom_filters_even_when_line_count_is_unchanged() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "alpha\nbeta\n")?;
    exec_git_cmd(&["add", "file.txt"], dir.path());
    exec_git_cmd(&["commit", "-m", "two lines"], dir.path());
    std::fs::write(
        dir.path().join(".gitattributes"),
        "file.txt filter=reorder\n",
    )?;
    exec_git_cmd(&["config", "filter.reorder.clean", "sort"], dir.path());
    exec_git_cmd(&["config", "filter.reorder.smudge", "cat"], dir.path());
    let result = GitRepository::discover(dir.path())?.prepare_partial(
        &path,
        FileOperation::Stage,
        &PartialSource::Buffer("zeta\nbeta\n".into()),
        PartialSelection::Lines(std::iter::once(0..1).collect()),
        true,
    );
    assert!(
        result.is_err(),
        "A custom filter cannot establish source line coordinates"
    );
    assert_eq!(staged_text(dir.path(), "file.txt"), "alpha\nbeta\n");
    Ok(())
}

#[tokio::test]
async fn partial_review_revalidates_disk_after_capture() -> Result<()> {
    for comparison in [
        crate::ComparisonKind::Unstaged,
        crate::ComparisonKind::HeadToWorktree,
    ] {
        let dir = initial();
        let path = dir.path().join("file.txt");
        let repo = GitRepository::discover(dir.path())?;
        std::fs::write(&path, "unseen\n")?;
        let source = PartialSource::Review {
            comparison,
            before: "head\n".into(),
            after: "shown\n".into(),
        };
        assert!(repo
            .prepare_partial(
                &path,
                FileOperation::Stage,
                &source,
                PartialSelection::Lines(std::iter::once(0..1).collect()),
                true
            )
            .is_err());
        assert_eq!(staged_text(dir.path(), "file.txt"), "head\n");
    }
    Ok(())
}

#[tokio::test]
async fn selecting_all_changes_stages_the_source_and_unstaging_restores_head() -> Result<()> {
    for (before, after) in [
        ("one\nkeep\nthree\n", "ONE\nkeep\nTHREE\n"),
        ("one\ntwo\n", "added\none\ntwo\ntail\n"),
        ("one\ntwo\nthree\n", "two\n"),
        ("before", "after\n"),
        ("α\n😀\n", "β\n😀\nγ"),
        ("before\n", ""),
    ] {
        let dir = initial();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, before)?;
        exec_git_cmd(&["add", "file.txt"], dir.path());
        exec_git_cmd(&["commit", "-m", "case"], dir.path());
        let repo = GitRepository::discover(dir.path())?;
        let all = PartialSelection::Hunks(std::iter::once(0..200_000).collect());
        repo.prepare_partial(
            &path,
            FileOperation::Stage,
            &PartialSource::Buffer(after.into()),
            all.clone(),
            true,
        )?
        .execute()
        .await?;
        assert_eq!(staged_text(dir.path(), "file.txt"), after);
        repo.prepare_partial(
            &path,
            FileOperation::Unstage,
            &PartialSource::Index,
            all,
            true,
        )?
        .execute()
        .await?;
        assert_eq!(staged_text(dir.path(), "file.txt"), before);
        assert_eq!(
            std::fs::read_to_string(&path)?,
            before,
            "Index actions must not write the file"
        );
    }
    Ok(())
}

#[tokio::test]
async fn partial_staging_preserves_existing_index_and_unsaved_disk_text() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "already staged\nsecond\nthird\n")?;
    exec_git_cmd(&["add", "file.txt"], dir.path());
    std::fs::write(&path, "disk stays untouched\n")?;
    let repo = GitRepository::discover(dir.path())?;
    let source = PartialSource::Buffer("already staged\nselected\nleave unsaved\n".into());
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &source,
        PartialSelection::Lines(std::iter::once(1..2).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(
        staged_text(dir.path(), "file.txt"),
        "already staged\nselected\nthird\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, "disk stays untouched\n");
    repo.prepare_partial(
        &path,
        FileOperation::Unstage,
        &source,
        PartialSelection::Lines(std::iter::once(1..2).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(
        staged_text(dir.path(), "file.txt"),
        "already staged\nthird\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, "disk stays untouched\n");
    Ok(())
}

#[tokio::test]
async fn partial_new_file_and_deletions_preserve_literal_names_and_eof() -> Result<()> {
    let dir = initial();
    std::fs::create_dir(dir.path().join("nested dir"))?;
    let path = dir.path().join("nested dir/[literal] file.txt");
    std::fs::write(&path, "first\nsecond\nthird")?;
    let repo = GitRepository::discover(dir.path())?;
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &PartialSource::Worktree,
        PartialSelection::Lines(std::iter::once(1..2).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(
        staged_text(dir.path(), "nested dir/[literal] file.txt"),
        "second\n"
    );
    repo.prepare_partial(
        &path,
        FileOperation::Unstage,
        &PartialSource::Index,
        PartialSelection::Hunks(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert!(statuses(dir.path())
        .into_iter()
        .find(|s| s.path() == path)
        .unwrap()
        .staged
        .is_none());
    assert_eq!(std::fs::read_to_string(&path)?, "first\nsecond\nthird");
    let tracked = dir.path().join("file.txt");
    repo.prepare_partial(
        &tracked,
        FileOperation::Stage,
        &PartialSource::Buffer(String::new()),
        PartialSelection::Hunks(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(dir.path(), "file.txt"), "");
    assert_eq!(std::fs::read_to_string(tracked)?, "head\n");
    Ok(())
}

#[tokio::test]
async fn partial_staging_rejects_stale_index_and_preserves_lock() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    let repo = GitRepository::discover(dir.path())?;
    let prepare = || {
        repo.prepare_partial(
            &path,
            FileOperation::Stage,
            &PartialSource::Buffer("selected\n".into()),
            PartialSelection::Hunks(std::iter::once(0..1).collect()),
            true,
        )
    };
    let plan = prepare()?;
    std::fs::write(&path, "external\n")?;
    exec_git_cmd(&["add", "file.txt"], dir.path());
    assert!(plan.execute().await.is_err());
    assert_eq!(staged_text(dir.path(), "file.txt"), "external\n");
    let plan = prepare()?;
    let lock = dir.path().join(".git/index.lock");
    std::fs::write(&lock, "another process")?;
    assert!(plan.execute().await.is_err());
    assert_eq!(std::fs::read_to_string(lock)?, "another process");
    assert_eq!(staged_text(dir.path(), "file.txt"), "external\n");
    assert!(repo
        .prepare_partial(
            &path,
            FileOperation::Stage,
            &PartialSource::Buffer("selected\n".into()),
            PartialSelection::Hunks(std::iter::once(0..1).collect()),
            false
        )
        .is_err());
    Ok(())
}

#[tokio::test]
async fn partial_staging_normalizes_crlf() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    std::fs::write(
        dir.path().join(".gitattributes"),
        "file.txt text eol=crlf\n",
    )?;
    let repo = GitRepository::discover(dir.path())?;
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &PartialSource::Buffer("selected\r\nsecond\r\n".into()),
        PartialSelection::Lines(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(dir.path(), "file.txt"), "selected\n");
    assert_eq!(std::fs::read_to_string(path)?, "head\n");
    Ok(())
}

#[tokio::test]
async fn partial_staging_linked_worktree_uses_its_own_index() -> Result<()> {
    let dir = initial();
    let worktree = tempfile::tempdir()?;
    exec_git_cmd(
        &[
            "worktree",
            "add",
            "--detach",
            worktree.path().to_str().unwrap(),
        ],
        dir.path(),
    );
    let path = worktree.path().join("file.txt");
    let repo = GitRepository::discover(worktree.path())?;
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &PartialSource::Buffer("worktree only\n".into()),
        PartialSelection::Hunks(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(worktree.path(), "file.txt"), "worktree only\n");
    assert_eq!(staged_text(dir.path(), "file.txt"), "head\n");
    Ok(())
}

#[tokio::test]
async fn partial_review_rows_select_removals_and_additions_independently() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    let repo = GitRepository::discover(dir.path())?;
    let source = PartialSource::Buffer("replacement\n".into());
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &source,
        PartialSelection::ReviewLines {
            before: vec![],
            after: std::iter::once(0..1).collect(),
        },
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(dir.path(), "file.txt"), "head\nreplacement\n");
    repo.prepare_partial(
        &path,
        FileOperation::Unstage,
        &PartialSource::Index,
        PartialSelection::ReviewLines {
            before: std::iter::once(1..2).collect(),
            after: vec![],
        },
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(dir.path(), "file.txt"), "head\n");
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &source,
        PartialSelection::ReviewLines {
            before: std::iter::once(0..1).collect(),
            after: vec![],
        },
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(dir.path(), "file.txt"), "");
    Ok(())
}

#[tokio::test]
async fn partial_unstaging_restores_a_staged_file_deletion() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    exec_git_cmd(&["rm", "file.txt"], dir.path());
    GitRepository::discover(dir.path())?
        .prepare_partial(
            &path,
            FileOperation::Unstage,
            &PartialSource::Index,
            PartialSelection::Hunks(std::iter::once(0..1).collect()),
            true,
        )?
        .execute()
        .await?;
    assert_eq!(staged_text(dir.path(), "file.txt"), "head\n");
    assert!(!path.exists());
    Ok(())
}

#[tokio::test]
async fn partial_staging_rejects_stale_review_and_ignored_files() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    let repo = GitRepository::discover(dir.path())?;
    std::fs::write(&path, "new index\n")?;
    exec_git_cmd(&["add", "file.txt"], dir.path());
    let stale = PartialSource::Review {
        comparison: crate::ComparisonKind::Index,
        before: "head\n".into(),
        after: "reviewed\n".into(),
    };
    assert!(repo
        .prepare_partial(
            &path,
            FileOperation::Stage,
            &stale,
            PartialSelection::Lines(std::iter::once(0..1).collect()),
            true
        )
        .is_err());
    assert_eq!(staged_text(dir.path(), "file.txt"), "new index\n");
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
    let ignored = dir.path().join("ignored.txt");
    std::fs::write(&ignored, "ignored\n")?;
    assert!(repo
        .prepare_partial(
            &ignored,
            FileOperation::Stage,
            &PartialSource::Worktree,
            PartialSelection::Lines(std::iter::once(0..1).collect()),
            true
        )
        .is_err());
    Ok(())
}

#[tokio::test]
async fn partial_staging_preserves_utf8_bom_and_staged_rename() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "\u{feff}head\nkeep one\nkeep two\nkeep three\n")?;
    create_commit_with_message(dir.path(), true, "with BOM");
    exec_git_cmd(&["mv", "file.txt", "renamed.txt"], dir.path());
    let path = dir.path().join("renamed.txt");
    let repo = GitRepository::discover(dir.path())?;
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &PartialSource::Buffer("\u{feff}selected\nkeep one\nkeep two\nkeep three\n".into()),
        PartialSelection::Lines(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(
        staged_text(dir.path(), "renamed.txt"),
        "\u{feff}selected\nkeep one\nkeep two\nkeep three\n"
    );
    repo.prepare_partial(
        &path,
        FileOperation::Unstage,
        &PartialSource::Index,
        PartialSelection::Hunks(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(
        staged_text(dir.path(), "renamed.txt"),
        "\u{feff}head\nkeep one\nkeep two\nkeep three\n"
    );
    assert!(!dir.path().join("file.txt").exists());
    Ok(())
}

#[tokio::test]
async fn partial_staging_untracked_review_snapshot_preserves_utf8_bom() -> Result<()> {
    let dir = initial();
    let path = dir.path().join("new.txt");
    std::fs::write(&path, "\u{feff}first\nsecond\n")?;
    let repo = GitRepository::discover(dir.path())?;
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &PartialSource::Review {
            comparison: crate::ComparisonKind::Unstaged,
            before: String::new(),
            after: "first\nsecond\n".into(),
        },
        PartialSelection::Lines(std::iter::once(1..2).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(dir.path(), "new.txt"), "\u{feff}second\n");
    let source = PartialSource::Buffer("\u{feff}first\nsecond\n".into());
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &source,
        PartialSelection::Lines(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(
        staged_text(dir.path(), "new.txt"),
        "\u{feff}first\nsecond\n"
    );
    repo.prepare_partial(
        &path,
        FileOperation::Unstage,
        &source,
        PartialSelection::Lines(std::iter::once(1..2).collect()),
        true,
    )?
    .execute()
    .await?;
    assert_eq!(staged_text(dir.path(), "new.txt"), "\u{feff}first\n");

    assert_eq!(std::fs::read_to_string(&path)?, "\u{feff}first\nsecond\n");
    std::fs::remove_file(&path)?;
    repo.prepare_partial(
        &path,
        FileOperation::Stage,
        &PartialSource::Worktree,
        PartialSelection::Hunks(std::iter::once(0..1).collect()),
        true,
    )?
    .execute()
    .await?;
    assert!(statuses(dir.path())
        .iter()
        .all(|status| status.path() != path));
    Ok(())
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
    let directory = tempfile::Builder::new().prefix("mitos git ").tempdir()?;
    let root = directory.path().join("repository with spaces");
    std::fs::create_dir(&root)?;
    exec_git_cmd(&["init"], &root);
    let root = root.canonicalize()?;
    let git = GitRepository::discover(&root)?;
    assert_eq!(git.root(), root);
    let name = "nested directory/[literal].txt";
    std::fs::create_dir(root.join("nested directory"))?;
    std::fs::write(root.join(name), "literal")?;
    std::fs::write(root.join("nested directory/l.txt"), "other")?;
    let status = statuses(&root)
        .into_iter()
        .find(|status| status.path().ends_with(name))
        .unwrap();
    git.prepare_file(&status, FileOperation::Stage, true)?
        .execute()
        .await?;
    let status = statuses(&root)
        .into_iter()
        .find(|status| status.path().ends_with(name))
        .unwrap();
    assert!(status.staged.is_some());
    assert!(statuses(&root)
        .iter()
        .find(|status| status.path().ends_with("l.txt"))
        .unwrap()
        .staged
        .is_none());
    git.prepare_file(&status, FileOperation::Unstage, true)?
        .execute()
        .await?;
    assert!(root.join(name).exists());
    assert!(statuses(&root).iter().all(|status| status.staged.is_none()));
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

#[tokio::test]
async fn stash_browsing_works_without_git_executable() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_STASH_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let repo = GitRepository::discover(Path::new(&root))?;
        let entries = repo.stashes().await?;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].selector, "stash@{0}");
        assert_eq!(entries[1].selector, "stash@{1}");
        assert!(entries[0].message.contains("Mitos: repository changes"));
        let files = repo.stash_files(&entries[0]).await?;
        let file = files
            .iter()
            .find(|file| file.layer() == StashLayer::Worktree)
            .unwrap();
        assert_eq!(
            repo.read_stash_file(&entries[0], file, false)?.after,
            b"newer\n"
        );
        let files = repo.stash_files(&entries[1]).await?;
        let file = files.iter().find(|file| file.untracked()).unwrap();
        assert_eq!(file.path(), Path::new("nested/new file.txt"));
        assert_eq!(
            repo.read_stash_file(&entries[1], file, false)?.after,
            b"untracked\n"
        );
        return Ok(());
    }
    let dir = initial();
    let repo = GitRepository::discover(dir.path())?;
    assert!(repo.stashes().await?.is_empty());
    std::fs::write(dir.path().join("file.txt"), "older\n")?;
    std::fs::create_dir(dir.path().join("nested"))?;
    std::fs::write(dir.path().join("nested/new file.txt"), "untracked\n")?;
    repo.prepare_stash(true)?.execute().await?;
    std::fs::write(dir.path().join("file.txt"), "newer\n")?;
    repo.prepare_stash(true)?.execute().await?;
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "operations::tests::stash_browsing_works_without_git_executable",
            "--nocapture",
        ])
        .env(FIXTURE, dir.path())
        .env("PATH", no_programs.path())
        .env("GIT_EXEC_PATH", no_programs.path())
        .output()?;
    ensure!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    Ok(())
}

#[tokio::test]
async fn unstaging_works_without_git_and_preserves_other_staged_files() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_UNSTAGE_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let repo = GitRepository::discover(Path::new(&root))?;
        let git = repo.open_read(true)?;
        let other = git
            .open_index()?
            .entry_by_path("other.txt".into())
            .unwrap()
            .id;
        let status = FileStatus {
            staged: Some(FileChange::Modified {
                path: repo.root.join("file.txt"),
            }),
            unstaged: None,
        };
        let plan = repo.prepare_file(&status, FileOperation::Unstage, true)?;
        let lock = git.index_path().with_extension("lock");
        std::fs::write(&lock, "another process")?;
        assert!(plan.execute().await.is_err());
        assert_eq!(std::fs::read_to_string(&lock)?, "another process");
        std::fs::remove_file(lock)?;
        repo.prepare_file(&status, FileOperation::Unstage, true)?
            .execute()
            .await?;
        let index = git.open_index()?;
        let tree = git.head_commit()?.tree()?;
        assert_eq!(
            index.entry_by_path("file.txt".into()).unwrap().id,
            tree.lookup_entry_by_path("file.txt")?.unwrap().object_id()
        );
        assert_eq!(index.entry_by_path("other.txt".into()).unwrap().id, other);
        assert_eq!(
            std::fs::read_to_string(repo.root.join("file.txt"))?,
            "worktree\n"
        );
        return Ok(());
    }
    let dir = initial();
    std::fs::write(dir.path().join("file.txt"), "index\n")?;
    std::fs::write(dir.path().join("other.txt"), "keep staged\n")?;
    exec_git_cmd(&["add", "."], dir.path());
    std::fs::write(dir.path().join("file.txt"), "worktree\n")?;
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "operations::tests::unstaging_works_without_git_and_preserves_other_staged_files",
            "--nocapture",
        ])
        .env(FIXTURE, dir.path())
        .env("PATH", no_programs.path())
        .env("GIT_EXEC_PATH", no_programs.path())
        .output()?;
    ensure!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    Ok(())
}

#[test]
fn native_directory_file_transitions_match_git_and_keep_unrelated_staged_edits() -> Result<()> {
    fn entries(
        repo: &gix::Repository,
    ) -> Result<Vec<(gix::bstr::BString, gix::ObjectId, gix::index::entry::Mode)>> {
        let index = repo.open_index()?;
        Ok(index
            .entries()
            .iter()
            .map(|entry| (entry.path(&index).to_owned(), entry.id, entry.mode))
            .collect())
    }
    let dir = initial();
    let root = dir.path();
    std::fs::create_dir_all(root.join("directory/nested"))?;
    std::fs::write(root.join("directory/nested/old.txt"), "old\n")?;
    std::fs::write(root.join("directory/peer.txt"), "peer\n")?;
    create_commit_with_message(root, true, "directory");
    std::fs::remove_dir_all(root.join("directory"))?;
    std::fs::write(root.join("directory"), "replacement\r\n")?;
    std::fs::write(root.join("other.txt"), "unrelated staged edit\n")?;
    exec_git_cmd(&["add", "other.txt"], root);
    let repo = GitRepository::discover(root)?;
    let git = repo.open_read(true)?;
    let original = std::fs::read(git.index_path())?;
    let change = FileChange::Renamed {
        from_path: root.join("directory/nested/old.txt"),
        to_path: root.join("directory"),
    };
    let status = FileStatus {
        staged: None,
        unstaged: Some(change.clone()),
    };
    // Calling the native entry points prevents a Git fallback from hiding missing support.
    assert!(repo
        .prepare_file(&status, FileOperation::Stage, true)?
        .stage()?
        .into_applied()
        .is_some());
    let native = entries(&git)?;
    std::fs::write(git.index_path(), original)?;
    exec_git_cmd(
        &[
            "add",
            "--all",
            "--",
            ":(literal)directory",
            ":(literal)directory/nested/old.txt",
        ],
        root,
    );
    assert_eq!(entries(&git)?, native);
    let staged = std::fs::read(git.index_path())?;
    let status = FileStatus {
        staged: Some(change),
        unstaged: None,
    };
    assert!(repo
        .prepare_file(&status, FileOperation::Unstage, true)?
        .unstage()?
        .into_applied()
        .is_some());
    let native = entries(&git)?;
    std::fs::write(git.index_path(), staged)?;
    exec_git_cmd(
        &[
            "reset",
            "--quiet",
            "HEAD",
            "--",
            ":(literal)directory",
            ":(literal)directory/nested/old.txt",
        ],
        root,
    );
    assert_eq!(entries(&git)?, native);
    assert_eq!(
        std::fs::read_to_string(root.join("directory"))?,
        "replacement\r\n"
    );
    assert_eq!(
        git.find_blob(
            git.open_index()?
                .entry_by_path("other.txt".into())
                .unwrap()
                .id
        )?
        .data,
        b"unrelated staged edit\n"
    );
    Ok(())
}

#[tokio::test]
async fn staging_without_git_applies_attributes_and_keeps_other_entries() -> Result<()> {
    const FIXTURE: &str = "MITOS_STAGE_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let repo = GitRepository::discover(Path::new(&root))?;
        let git = repo.open_read(true)?;
        let other = git
            .open_index()?
            .entry_by_path("other.txt".into())
            .unwrap()
            .id;
        for path in ["file.txt", "new.txt"] {
            let status = FileStatus {
                staged: None,
                unstaged: Some(FileChange::Modified {
                    path: repo.root.join(path),
                }),
            };
            repo.prepare_file(&status, FileOperation::Stage, true)?
                .execute()
                .await?;
        }
        let index = git.open_index()?;
        assert_eq!(index.entry_by_path("other.txt".into()).unwrap().id, other);
        assert_eq!(
            git.find_blob(index.entry_by_path("file.txt".into()).unwrap().id)?
                .data,
            b"changed\n"
        );
        assert_eq!(
            git.find_blob(index.entry_by_path("new.txt".into()).unwrap().id)?
                .data,
            b"new\n"
        );
        assert_eq!(std::fs::read(repo.root.join("file.txt"))?, b"changed\r\n");
        return Ok(());
    }
    let dir = initial();
    std::fs::write(dir.path().join("file.txt"), "changed\r\n")?;
    std::fs::write(dir.path().join("new.txt"), "new\r\n")?;
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "operations::tests::staging_without_git_applies_attributes_and_keeps_other_entries",
            "--nocapture",
        ])
        .env(FIXTURE, dir.path())
        .env("PATH", no_programs.path())
        .env("GIT_EXEC_PATH", no_programs.path())
        .output()?;
    ensure!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    Ok(())
}

#[test]
fn native_stage_keeps_hidden_descendants_when_a_directory_becomes_a_file() -> Result<()> {
    let dir = initial();
    let root = dir.path();
    std::fs::create_dir(root.join("directory"))?;
    std::fs::write(root.join("directory/hidden.txt"), "hidden\n")?;
    create_commit_with_message(root, true, "directory");
    std::fs::remove_dir_all(root.join("directory"))?;
    std::fs::write(root.join("directory"), "replacement\n")?;
    let repo = GitRepository::discover(root)?;
    let git = repo.open_read(true)?;
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Added {
            path: root.join("directory"),
        }),
    };
    for (enable, disable) in [
        ("--assume-unchanged", "--no-assume-unchanged"),
        ("--skip-worktree", "--no-skip-worktree"),
    ] {
        exec_git_cmd(&["update-index", enable, "directory/hidden.txt"], root);
        let before = std::fs::read(git.index_path())?;
        assert!(repo
            .prepare_file(&status, FileOperation::Stage, true)?
            .stage()?
            .into_applied()
            .is_none());
        assert_eq!(std::fs::read(git.index_path())?, before);
        exec_git_cmd(&["update-index", disable, "directory/hidden.txt"], root);
    }
    assert_eq!(
        std::fs::read_to_string(root.join("directory"))?,
        "replacement\n"
    );
    Ok(())
}

#[test]
fn native_file_to_directory_stages_only_selected_descendants() -> Result<()> {
    let dir = initial();
    let root = dir.path();
    std::fs::write(root.join(".gitignore"), "ignored.txt\n")?;
    create_commit_with_message(root, true, "ignore");
    std::fs::remove_file(root.join("file.txt"))?;
    std::fs::create_dir(root.join("file.txt"))?;
    std::fs::write(root.join("file.txt/selected.txt"), "selected\r\n")?;
    std::fs::write(root.join("file.txt/peer.txt"), "peer\n")?;
    std::fs::write(root.join("file.txt/ignored.txt"), "ignored\n")?;
    std::fs::write(root.join("other.txt"), "unrelated staged\n")?;
    exec_git_cmd(&["add", "other.txt"], root);
    let repo = GitRepository::discover(root)?;
    let git = repo.open_read(true)?;
    let status = |path: &str| FileStatus {
        staged: None,
        unstaged: Some(FileChange::Added {
            path: root.join(path),
        }),
    };
    for (enable, disable) in [
        ("--assume-unchanged", "--no-assume-unchanged"),
        ("--skip-worktree", "--no-skip-worktree"),
    ] {
        exec_git_cmd(&["update-index", enable, "file.txt"], root);
        let before = std::fs::read(git.index_path())?;
        assert!(repo
            .prepare_file(&status("file.txt/selected.txt"), FileOperation::Stage, true)?
            .stage()?
            .into_applied()
            .is_none());
        assert_eq!(std::fs::read(git.index_path())?, before);
        exec_git_cmd(&["update-index", disable, "file.txt"], root);
    }
    let before = std::fs::read(git.index_path())?;
    assert!(repo
        .prepare_file(&status("file.txt/ignored.txt"), FileOperation::Stage, true)?
        .stage()
        .is_err());
    assert_eq!(std::fs::read(git.index_path())?, before);
    assert!(repo
        .prepare_file(&status("file.txt/selected.txt"), FileOperation::Stage, true)?
        .stage()?
        .into_applied()
        .is_some());
    let entries = || -> Result<Vec<_>> {
        let index = git.open_index()?;
        Ok(index
            .entries()
            .iter()
            .map(|entry| (entry.path(&index).to_owned(), entry.id, entry.mode))
            .collect())
    };
    let native = entries()?;
    std::fs::write(git.index_path(), before)?;
    exec_git_cmd(
        &["add", "--all", "--", ":(literal)file.txt/selected.txt"],
        root,
    );
    assert_eq!(entries()?, native);
    let index = git.open_index()?;
    assert!(index.entry_by_path("file.txt".into()).is_none());
    assert!(index.entry_by_path("file.txt/peer.txt".into()).is_none());
    assert!(index.entry_by_path("file.txt/ignored.txt".into()).is_none());
    assert_eq!(
        git.find_blob(
            index
                .entry_by_path("file.txt/selected.txt".into())
                .unwrap()
                .id
        )?
        .data,
        b"selected\n"
    );
    assert_eq!(
        git.find_blob(index.entry_by_path("other.txt".into()).unwrap().id)?
            .data,
        b"unrelated staged\n"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn failed_native_filters_are_not_retried_with_git() -> Result<()> {
    let dir = initial();
    let root = dir.path();
    std::fs::write(root.join(".gitattributes"), "file.txt filter=once\n")?;
    exec_git_cmd(
        &[
            "config",
            "filter.once.clean",
            "echo called >> filter-calls; exit 1",
        ],
        root,
    );
    exec_git_cmd(&["config", "filter.once.required", "true"], root);
    std::fs::write(root.join("file.txt"), "local changes\n")?;
    let repo = GitRepository::discover(root)?;
    let git = repo.open_read(true)?;
    let before = std::fs::read(git.index_path())?;
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified {
            path: root.join("file.txt"),
        }),
    };
    assert!(repo
        .prepare_file(&status, FileOperation::Stage, true)?
        .execute()
        .await
        .is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("filter-calls"))?,
        "called\n"
    );
    assert_eq!(std::fs::read(git.index_path())?, before);
    assert_eq!(
        std::fs::read_to_string(root.join("file.txt"))?,
        "local changes\n"
    );
    Ok(())
}

#[tokio::test]
async fn global_attribute_changes_invalidate_prepared_operations() -> Result<()> {
    let repo = initial();
    let external = tempfile::tempdir()?;
    let attributes = external.path().join("global attributes");
    exec_git_cmd(
        &[
            "config",
            "core.attributesFile",
            attributes.to_str().unwrap(),
        ],
        repo.path(),
    );
    let git = GitRepository::discover(repo.path())?;
    std::fs::write(repo.path().join("file.txt"), "new\n")?;
    for contents in ["*.txt text eol=lf\n", "*.txt -text\n"] {
        let status = statuses(repo.path()).remove(0);
        let plan = git.prepare_file(&status, FileOperation::Stage, true)?;
        std::fs::write(&attributes, contents)?;
        assert!(plan
            .execute()
            .await
            .unwrap_err()
            .to_string()
            .contains("Git state changed"));
        assert!(statuses(repo.path())[0].staged.is_none());
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn linked_global_attribute_targets_invalidate_prepared_operations() -> Result<()> {
    let repo = initial();
    let external = tempfile::tempdir()?;
    let target = external.path().join("attributes-source");
    let attributes = external.path().join("global-attributes");
    std::os::unix::fs::symlink(&target, &attributes)?;
    exec_git_cmd(
        &[
            "config",
            "core.attributesFile",
            attributes.to_str().unwrap(),
        ],
        repo.path(),
    );
    let git = GitRepository::discover(repo.path())?;
    std::fs::write(repo.path().join("file.txt"), "new\n")?;
    // Both creating a missing target and editing an existing target leave the link unchanged.
    for contents in ["*.txt text eol=lf\n", "*.txt -text\n"] {
        let plan =
            git.prepare_file(&statuses(repo.path()).remove(0), FileOperation::Stage, true)?;
        std::fs::write(&target, contents)?;
        assert!(plan
            .execute()
            .await
            .unwrap_err()
            .to_string()
            .contains("Git state changed"));
        assert!(statuses(repo.path())[0].staged.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn commit_records_only_the_reviewed_index_and_keeps_the_worktree() -> Result<()> {
    let repo = initial();
    let path = repo.path().join("file.txt");
    std::fs::write(&path, "staged\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    let git = GitRepository::discover(repo.path())?;
    let snapshot = git.commit_snapshot(true)?;
    std::fs::write(&path, "unstaged\n")?;
    let operation = snapshot.prepare("Focused commit\n\nDetails.\n", true)?;
    assert!(!operation.requires_clean_buffers());
    let Operation::Commit(file) = &operation.operation else {
        unreachable!()
    };
    let message_file = file.path().to_owned();
    operation.execute().await?;
    assert!(!message_file.exists());
    let committed = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["show", "HEAD:file.txt"])
        .output()?;
    assert!(committed.status.success());
    assert_eq!(committed.stdout, b"staged\n");
    assert_eq!(std::fs::read_to_string(&path)?, "unstaged\n");
    assert!(statuses(repo.path())
        .iter()
        .all(|status| status.staged.is_none()));
    Ok(())
}

#[tokio::test]
async fn commit_requires_trust_staged_changes_and_the_reviewed_index() -> Result<()> {
    let repo = initial();
    let git = GitRepository::discover(repo.path())?;
    assert!(git.commit_snapshot(false).is_err());
    assert!(git.commit_snapshot(true).is_err());
    std::fs::write(repo.path().join("file.txt"), "staged\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    let snapshot = git.commit_snapshot(true)?;
    assert!(snapshot.prepare("message", false).is_err());
    assert!(snapshot.prepare(" \n", true).is_err());
    std::fs::write(repo.path().join("other.txt"), "another staged change\n")?;
    exec_git_cmd(&["add", "other.txt"], repo.path());
    assert!(snapshot.prepare("message", true).is_err());
    Ok(())
}

#[tokio::test]
async fn commit_creates_the_first_commit_from_staged_files() -> Result<()> {
    let repo = empty_git_repo();
    std::fs::write(repo.path().join("first.txt"), "first\n")?;
    exec_git_cmd(&["add", "first.txt"], repo.path());
    let git = GitRepository::discover(repo.path())?;
    git.commit_snapshot(true)?
        .prepare("initial commit", true)?
        .execute()
        .await?;
    assert!(!git.open(true)?.head()?.is_unborn());
    assert!(statuses(repo.path()).is_empty());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn commit_reports_hook_and_signing_failures_without_advancing_head() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let repo = initial();
    std::fs::write(repo.path().join("file.txt"), "staged\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    let hook = repo.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho 'hook rejected' >&2\nexit 1\n")?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    let git = GitRepository::discover(repo.path())?;
    let operation = git
        .commit_snapshot(true)?
        .prepare("keep this message", true)?;
    let result = operation.execute().await;
    assert!(result.unwrap_err().to_string().contains("hook rejected"));
    assert!(statuses(repo.path())
        .iter()
        .any(|status| status.staged.is_some()));
    std::fs::remove_file(hook)?;
    exec_git_cmd(&["config", "commit.gpgsign", "true"], repo.path());
    exec_git_cmd(
        &["config", "gpg.program", "/mitos-nonexistent-gpg"],
        repo.path(),
    );
    assert!(git
        .commit_snapshot(true)?
        .prepare("keep this message", true)?
        .execute()
        .await
        .is_err());
    assert!(statuses(repo.path())
        .iter()
        .any(|status| status.staged.is_some()));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn commit_pins_reviewed_index_and_preserves_external_staging_during_hooks() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    let repo = initial();
    std::fs::write(repo.path().join("file.txt"), "reviewed\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    let operation = GitRepository::discover(repo.path())?
        .commit_snapshot(true)?
        .prepare("reviewed commit", true)?;
    let hook = repo.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\ntouch .commit-ready\nattempt=0\nwhile [ ! -e .commit-release ] && [ \"$attempt\" -lt 500 ]; do attempt=$((attempt+1)); sleep 0.01; done\n")?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    let task = tokio::spawn(operation.execute_with_notice());
    tokio::time::timeout(Duration::from_secs(10), async {
        while !repo.path().join(".commit-ready").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    std::fs::write(repo.path().join("other.txt"), "newer external staging\n")?;
    exec_git_cmd(&["add", "other.txt"], repo.path());
    let external_index = std::fs::read(repo.path().join(".git/index"))?;
    std::fs::write(repo.path().join(".commit-release"), "continue")?;
    let notice = tokio::time::timeout(Duration::from_secs(10), task).await???;
    assert!(notice.is_some_and(|notice| notice.contains("Newer staging")));
    assert_eq!(
        std::fs::read(repo.path().join(".git/index"))?,
        external_index
    );
    let committed = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["show", "HEAD:other.txt"])
        .output()?;
    assert!(committed.status.success());
    assert_eq!(committed.stdout, b"other\n");
    assert!(statuses(repo.path())
        .iter()
        .any(|status| status.path().ends_with("other.txt") && status.staged.is_some()));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn commit_preserves_trusted_hook_index_updates_and_existing_index_locks() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let repo = initial();
    std::fs::write(repo.path().join("file.txt"), "reviewed\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    let hook = repo.path().join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf 'formatted by hook\\n' > file.txt\ngit add file.txt\n",
    )?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    let git = GitRepository::discover(repo.path())?;
    let index_path = repo.path().join(".git/index");
    std::fs::set_permissions(&index_path, std::fs::Permissions::from_mode(0o660))?;
    // Hook-staged work survives a failed attempt just as it does with ordinary Git.
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf 'formatted by hook\\n' > file.txt\ngit add file.txt\nexit 1\n",
    )?;
    assert!(git
        .commit_snapshot(true)?
        .prepare("failed hook", true)?
        .execute()
        .await
        .is_err());
    let staged = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["show", ":file.txt"])
        .output()?;
    assert!(staged.status.success());
    assert_eq!(staged.stdout, b"formatted by hook\n");
    assert_eq!(
        std::fs::metadata(&index_path)?.permissions().mode() & 0o777,
        0o660
    );
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf 'formatted by hook\\n' > file.txt\ngit add file.txt\n",
    )?;
    let notice = git
        .commit_snapshot(true)?
        .prepare("formatted commit", true)?
        .execute_with_notice()
        .await?;
    assert!(notice.is_none());
    assert!(statuses(repo.path()).is_empty());
    assert_eq!(
        std::fs::read_to_string(repo.path().join("file.txt"))?,
        "formatted by hook\n"
    );
    std::fs::remove_file(hook)?;
    std::fs::write(repo.path().join("file.txt"), "another change\n")?;
    exec_git_cmd(&["add", "file.txt"], repo.path());
    let operation = git.commit_snapshot(true)?.prepare("lock kept", true)?;
    let index = std::fs::read(repo.path().join(".git/index"))?;
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "another process")?;
    let notice = operation.execute_with_notice().await?;
    assert!(notice.is_some_and(|notice| notice.contains("could not be locked")));
    assert_eq!(std::fs::read_to_string(lock)?, "another process");
    assert_eq!(std::fs::read(repo.path().join(".git/index"))?, index);
    Ok(())
}

#[tokio::test]
async fn commit_private_index_preserves_merge_and_cherry_pick_state() -> Result<()> {
    let repo = initial();
    exec_git_cmd(&["checkout", "-b", "feature"], repo.path());
    std::fs::write(repo.path().join("file.txt"), "feature\n")?;
    create_commit_with_message(repo.path(), true, "feature");
    exec_git_cmd(&["checkout", "main"], repo.path());
    std::fs::write(repo.path().join("other.txt"), "main\n")?;
    create_commit_with_message(repo.path(), true, "main");
    exec_git_cmd(&["merge", "--no-commit", "--no-ff", "feature"], repo.path());
    let git = GitRepository::discover(repo.path())?;
    assert!(git.git_dir.join("MERGE_HEAD").exists());
    git.commit_snapshot(true)?
        .prepare("merge feature", true)?
        .execute()
        .await?;
    assert_eq!(git.open(true)?.head_commit()?.parent_ids().count(), 2);
    assert!(!git.git_dir.join("MERGE_HEAD").exists());
    assert!(statuses(repo.path()).is_empty());

    exec_git_cmd(&["checkout", "-b", "pick-source"], repo.path());
    std::fs::write(repo.path().join("other.txt"), "picked\n")?;
    create_commit_with_message(repo.path(), true, "pick source");
    let pick = git.open(true)?.head_commit()?.id.to_string();
    exec_git_cmd(&["checkout", "main"], repo.path());
    std::fs::write(repo.path().join("other.txt"), "conflicting main\n")?;
    create_commit_with_message(repo.path(), true, "conflicting main");
    let failed = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["cherry-pick", &pick])
        .env_remove("GIT_DIR")
        .output()?;
    assert!(!failed.status.success());
    assert!(git.git_dir.join("CHERRY_PICK_HEAD").exists());
    std::fs::write(repo.path().join("other.txt"), "resolved\n")?;
    exec_git_cmd(&["add", "other.txt"], repo.path());
    git.commit_snapshot(true)?
        .prepare("resolved cherry pick", true)?
        .execute()
        .await?;
    assert_eq!(git.open(true)?.head_commit()?.parent_ids().count(), 1);
    assert!(!git.git_dir.join("CHERRY_PICK_HEAD").exists());
    assert!(statuses(repo.path()).is_empty());
    Ok(())
}
