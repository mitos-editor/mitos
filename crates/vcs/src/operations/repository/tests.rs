use super::*;
use crate::git::test::{create_commit_with_message, empty_git_repo, exec_git_cmd};

fn initial() -> Result<tempfile::TempDir> {
    let dir = empty_git_repo();
    std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
    std::fs::write(dir.path().join("file.txt"), "initial\n")?;
    create_commit_with_message(dir.path(), true, "initial");
    Ok(dir)
}

async fn execute(repo: &GitRepository, operation: RepositoryOperation) -> Result<()> {
    repo.prepare_repository(operation, true)?.execute().await
}

#[tokio::test]
async fn branches_switch_without_losing_dirty_work_and_reject_untrusted_or_stale_actions(
) -> Result<()> {
    let dir = initial()?;
    let repo = GitRepository::discover(dir.path())?;
    execute(
        &repo,
        RepositoryOperation::CreateBranch {
            name: "topic/one".into(),
        },
    )
    .await?;
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic/one" && !branch.current));
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "topic/one".into(),
        },
    )
    .await?;
    std::fs::write(dir.path().join("file.txt"), "dirty\n")?;
    std::fs::write(dir.path().join("untracked.txt"), "keep\n")?;
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "main".into(),
        },
    )
    .await?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "dirty\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("untracked.txt"))?,
        "keep\n"
    );
    assert!(repo
        .prepare_repository(
            RepositoryOperation::CreateBranch {
                name: "--force".into()
            },
            true
        )
        .is_err());
    assert!(repo
        .prepare_repository(
            RepositoryOperation::CreateBranch {
                name: "untrusted".into()
            },
            false
        )
        .is_err());
    let plan = repo.prepare_repository(
        RepositoryOperation::SwitchBranch {
            name: "topic/one".into(),
        },
        true,
    )?;
    exec_git_cmd(&["config", "mitos.state", "changed"], dir.path());
    assert!(plan.execute().await.is_err());
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "main" && branch.current));
    Ok(())
}

#[tokio::test]
async fn linked_worktrees_can_be_created_and_only_clean_unlocked_noncurrent_trees_are_removed(
) -> Result<()> {
    let dir = initial()?;
    let parent = tempfile::tempdir()?;
    let linked = parent.path().join("linked tree");
    let repo = GitRepository::discover(dir.path())?;
    execute(
        &repo,
        RepositoryOperation::CreateBranch {
            name: "linked".into(),
        },
    )
    .await?;
    execute(
        &repo,
        RepositoryOperation::CreateWorktree {
            path: linked.clone(),
            branch: Some("linked".into()),
        },
    )
    .await?;
    let trees = repo.worktrees().await?;
    assert_eq!(trees.len(), 2);
    assert!(trees
        .iter()
        .any(|tree| !tree.main && tree.branch.as_deref() == Some("linked")));
    let linked_repo = GitRepository::discover(&linked)?;
    assert!(linked_repo
        .prepare_repository(
            RepositoryOperation::CreateWorktree {
                path: repo.common_dir().join("new-worktree"),
                branch: None
            },
            true
        )
        .is_err());
    assert!(linked_repo
        .prepare_repository(
            RepositoryOperation::RemoveWorktree {
                path: linked.clone()
            },
            true
        )
        .is_err());
    std::fs::write(linked.join("untracked.txt"), "preserved\n")?;
    assert!(execute(
        &repo,
        RepositoryOperation::RemoveWorktree {
            path: linked.clone()
        }
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read_to_string(linked.join("untracked.txt"))?,
        "preserved\n"
    );
    std::fs::remove_file(linked.join("untracked.txt"))?;
    std::fs::write(linked.join(".gitignore"), "ignored.txt\n")?;
    create_commit_with_message(&linked, true, "ignore file");
    std::fs::write(linked.join("ignored.txt"), "preserved ignored content\n")?;
    assert!(execute(
        &repo,
        RepositoryOperation::RemoveWorktree {
            path: linked.clone()
        }
    )
    .await
    .is_err());
    assert!(linked.join("ignored.txt").is_file());
    std::fs::remove_file(linked.join("ignored.txt"))?;
    exec_git_cmd(&["worktree", "lock", linked.to_str().unwrap()], dir.path());
    assert!(execute(
        &repo,
        RepositoryOperation::RemoveWorktree {
            path: linked.clone()
        }
    )
    .await
    .is_err());
    exec_git_cmd(
        &["worktree", "unlock", linked.to_str().unwrap()],
        dir.path(),
    );
    execute(
        &repo,
        RepositoryOperation::RemoveWorktree {
            path: linked.clone(),
        },
    )
    .await?;
    assert!(!linked.exists());
    assert_eq!(repo.worktrees().await?.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "initial\n"
    );
    Ok(())
}

#[tokio::test]
async fn explicit_remote_operations_use_local_bare_repos_and_never_force_or_merge() -> Result<()> {
    let dir = initial()?;
    let remote_dir = tempfile::tempdir()?;
    let remote = remote_dir.path().join("remote.git");
    exec_git_cmd(&["branch", "keep"], dir.path());
    exec_git_cmd(
        &[
            "clone",
            "--bare",
            dir.path().to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
        dir.path(),
    );
    exec_git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        dir.path(),
    );
    let repo = GitRepository::discover(dir.path())?;
    std::fs::write(dir.path().join("file.txt"), "local\n")?;
    create_commit_with_message(dir.path(), true, "local change");
    exec_git_cmd(&["config", "remote.origin.mirror", "true"], dir.path());
    execute(&repo, RepositoryOperation::Push { remote: None }).await?;
    exec_git_cmd(&["show-ref", "--verify", "refs/heads/keep"], &remote);
    let peer = remote_dir.path().join("peer");
    exec_git_cmd(
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
        dir.path(),
    );
    std::fs::write(peer.join("file.txt"), "remote\n")?;
    create_commit_with_message(&peer, true, "remote change");
    exec_git_cmd(&["push", "origin", "main"], &peer);
    execute(&repo, RepositoryOperation::Fetch { remote: None }).await?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "local\n"
    );
    exec_git_cmd(&["config", "pull.autoStash", "true"], dir.path());
    std::fs::write(dir.path().join("file.txt"), "dirty local\n")?;
    assert!(execute(&repo, RepositoryOperation::Pull { remote: None })
        .await
        .is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "dirty local\n"
    );
    assert!(
        repo.stashes().await?.is_empty(),
        "Pull must not create an implicit stash"
    );
    std::fs::write(dir.path().join("file.txt"), "local\n")?;
    execute(&repo, RepositoryOperation::Pull { remote: None }).await?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "remote\n"
    );
    std::fs::write(dir.path().join("local.txt"), "local branch\n")?;
    create_commit_with_message(dir.path(), true, "divergent local");
    std::fs::write(peer.join("remote.txt"), "remote branch\n")?;
    create_commit_with_message(&peer, true, "divergent remote");
    exec_git_cmd(&["push", "origin", "main"], &peer);
    assert!(execute(&repo, RepositoryOperation::Pull { remote: None })
        .await
        .is_err());
    assert!(execute(&repo, RepositoryOperation::Push { remote: None })
        .await
        .is_err());
    assert!(
        repo.open(false)?.state().is_none(),
        "Fast-forward-only pull must not begin a merge"
    );
    assert!(dir.path().join("local.txt").is_file());
    Ok(())
}

#[tokio::test]
async fn remote_selection_and_errors_expose_names_without_credentials() -> Result<()> {
    let dir = initial()?;
    exec_git_cmd(
        &[
            "remote",
            "add",
            "broken",
            "unavailable-protocol://credential-token@example.invalid/repo",
        ],
        dir.path(),
    );
    let repo = GitRepository::discover(dir.path())?;
    let error = execute(&repo, RepositoryOperation::Fetch { remote: None })
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("credential-token") && !error.contains("example.invalid"));
    exec_git_cmd(
        &[
            "remote",
            "add",
            "second",
            "another-unavailable-protocol://other-token@example.invalid/repo",
        ],
        dir.path(),
    );
    let error = repo
        .prepare_repository(RepositoryOperation::Fetch { remote: None }, true)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("broken") && error.contains("second"));
    assert!(!error.contains("token"));
    Ok(())
}

#[tokio::test]
async fn branch_switch_keeps_ignored_files_that_the_destination_starts_tracking() -> Result<()> {
    let dir = initial()?;
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\ncache.txt\n")?;
    create_commit_with_message(dir.path(), true, "ignore local files");
    exec_git_cmd(&["switch", "-c", "with-file"], dir.path());
    std::fs::write(dir.path().join("ignored.txt"), "tracked branch contents\n")?;
    exec_git_cmd(&["add", "-f", "ignored.txt"], dir.path());
    create_commit_with_message(dir.path(), false, "start tracking ignored path");
    exec_git_cmd(&["switch", "main"], dir.path());
    std::fs::write(
        dir.path().join("ignored.txt"),
        "precious ignored contents\n",
    )?;
    std::fs::write(dir.path().join("cache.txt"), "keep cache\n")?;
    let repo = GitRepository::discover(dir.path())?;
    assert!(execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "with-file".into()
        }
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ignored.txt"))?,
        "precious ignored contents\n"
    );
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "main" && branch.current));
    // Ordinary ignored build/cache files do not prevent switching branches.
    std::fs::remove_file(dir.path().join("ignored.txt"))?;
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "with-file".into(),
        },
    )
    .await?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("cache.txt"))?,
        "keep cache\n"
    );
    Ok(())
}

#[tokio::test]
async fn pull_keeps_ignored_files_that_upstream_starts_tracking() -> Result<()> {
    let dir = initial()?;
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\ncache.txt\n")?;
    create_commit_with_message(dir.path(), true, "ignore local files");
    let temporary = tempfile::tempdir()?;
    let remote = temporary.path().join("remote.git");
    let peer = temporary.path().join("peer");
    exec_git_cmd(
        &[
            "clone",
            "--bare",
            dir.path().to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
        dir.path(),
    );
    exec_git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        dir.path(),
    );
    exec_git_cmd(
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
        dir.path(),
    );
    std::fs::write(peer.join("ignored.txt"), "new tracked upstream contents\n")?;
    exec_git_cmd(&["add", "-f", "ignored.txt"], &peer);
    create_commit_with_message(&peer, false, "start tracking ignored file");
    exec_git_cmd(&["push", "origin", "main"], &peer);
    std::fs::write(
        dir.path().join("ignored.txt"),
        "precious ignored contents\n",
    )?;
    std::fs::write(dir.path().join("cache.txt"), "keep cache\n")?;
    let repo = GitRepository::discover(dir.path())?;
    let head = current_head(&repo.open(false)?)?;
    assert!(execute(&repo, RepositoryOperation::Pull { remote: None })
        .await
        .is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ignored.txt"))?,
        "precious ignored contents\n"
    );
    assert_eq!(current_head(&repo.open(false)?)?, head);
    assert!(repo.stashes().await?.is_empty());
    std::fs::remove_file(dir.path().join("ignored.txt"))?;
    execute(&repo, RepositoryOperation::Pull { remote: None }).await?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ignored.txt"))?,
        "new tracked upstream contents\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("cache.txt"))?,
        "keep cache\n"
    );
    Ok(())
}

#[tokio::test]
async fn worktree_removal_preserves_modifications_hidden_by_index_flags() -> Result<()> {
    let dir = initial()?;
    let temporary = tempfile::tempdir()?;
    let linked = temporary.path().join("linked");
    let repo = GitRepository::discover(dir.path())?;
    execute(
        &repo,
        RepositoryOperation::CreateWorktree {
            path: linked.clone(),
            branch: None,
        },
    )
    .await?;
    for (enable, disable) in [
        ("--assume-unchanged", "--no-assume-unchanged"),
        ("--skip-worktree", "--no-skip-worktree"),
    ] {
        exec_git_cmd(&["update-index", enable, "file.txt"], &linked);
        std::fs::write(linked.join("file.txt"), "precious hidden modification\n")?;
        let error = execute(
            &repo,
            RepositoryOperation::RemoveWorktree {
                path: linked.clone(),
            },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("assume-unchanged or skip-worktree"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(linked.join("file.txt"))?,
            "precious hidden modification\n"
        );
        exec_git_cmd(&["update-index", disable, "file.txt"], &linked);
        std::fs::write(linked.join("file.txt"), "initial\n")?;
    }
    execute(
        &repo,
        RepositoryOperation::RemoveWorktree {
            path: linked.clone(),
        },
    )
    .await?;
    assert!(!linked.exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn pull_revalidates_the_branch_after_fetch_before_any_checkout() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = initial()?;
    exec_git_cmd(&["branch", "other"], dir.path());
    let temporary = tempfile::tempdir()?;
    let remote = temporary.path().join("remote.git");
    exec_git_cmd(
        &[
            "clone",
            "--bare",
            dir.path().to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
        dir.path(),
    );
    exec_git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        dir.path(),
    );
    let source = dir.path().to_str().unwrap().replace('\'', "'\\''");
    let helper = dir.path().join(".git/switch-during-fetch");
    std::fs::write(&helper, format!("#!/bin/sh\ngit --git-dir='{source}/.git' --work-tree='{source}' switch --quiet other || exit\nexec git upload-pack \"$@\"\n"))?;
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))?;
    let helper_command = format!("'{}'", helper.to_str().unwrap().replace('\'', "'\\''"));
    exec_git_cmd(
        &["config", "remote.origin.uploadpack", &helper_command],
        dir.path(),
    );
    let repo = GitRepository::discover(dir.path())?;
    let error = execute(&repo, RepositoryOperation::Pull { remote: None })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("branch or HEAD changed during fetch"),
        "{error}"
    );
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "other" && branch.current));
    assert!(repo.open(false)?.state().is_none());
    Ok(())
}

#[tokio::test]
async fn repository_lists_work_without_git_executable() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_REPOSITORY_LISTS_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let repo = GitRepository::discover(Path::new(&root))?;
        let branches = repo.branches().await?;
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].name, "main");
        assert!(branches[0].current);
        assert_eq!(branches[0].upstream, "origin/main");
        assert_eq!(branches[1].name, "topic");
        assert!(!branches[1].current);
        let trees = repo.worktrees().await?;
        assert_eq!(trees.len(), 2);
        assert!(trees[0].main);
        assert_eq!(trees[1].branch.as_deref(), Some("topic"));
        assert!(trees[1].locked);
        assert!(!trees[1].prunable);
        assert_eq!(
            GitRepository::discover(&trees[1].path)?.worktrees().await?,
            trees
        );
        return Ok(());
    }
    let dir = initial()?;
    let root = dir.path();
    let linked = tempfile::tempdir()?;
    let path = linked.path().join("linked tree");
    exec_git_cmd(&["branch", "topic"], root);
    exec_git_cmd(&["remote", "add", "origin", "."], root);
    exec_git_cmd(&["update-ref", "refs/remotes/origin/main", "HEAD"], root);
    exec_git_cmd(&["branch", "--set-upstream-to=origin/main", "main"], root);
    exec_git_cmd(&["pack-refs", "--all"], root);
    exec_git_cmd(&["worktree", "add", path.to_str().unwrap(), "topic"], root);
    exec_git_cmd(&["worktree", "lock", path.to_str().unwrap()], root);
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "operations::repository::tests::repository_lists_work_without_git_executable",
            "--nocapture",
        ])
        .env(FIXTURE, root)
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
fn fetched_commit_is_pinned_and_rejects_noncommits() -> Result<()> {
    let dir = initial()?;
    let repository = GitRepository::discover(dir.path())?;
    let repo = repository.open_read(false)?;
    let id = repo.head_id()?;
    let path = repo.git_dir().join("FETCH_HEAD");
    std::fs::write(&path, format!("{id}\t\tbranch 'main' of local-fixture\n"))?;
    let pinned = fetched_commit(&repo)?;
    assert_eq!(pinned.as_str(), id.to_string());
    std::fs::write(&path, format!("{}\t\tinvalid tree\n", repo.head_tree_id()?))?;
    assert!(fetched_commit(&repo).is_err());
    assert_eq!(pinned.as_str(), id.to_string());
    Ok(())
}

#[tokio::test]
async fn worktree_safety_checks_do_not_need_git() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_WORKTREE_SAFETY_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let root = PathBuf::from(root);
        let owner = GitRepository::discover(&root)?;
        let linked = stdx::path::canonicalize_existing(&root.parent().unwrap().join("linked"));
        ensure_clean_worktree(&linked, &owner)?;
        std::fs::write(linked.join("ignored.txt"), "keep\n")?;
        assert!(ensure_clean_worktree(&linked, &owner).is_err());
        std::fs::remove_file(linked.join("ignored.txt"))?;
        std::fs::write(linked.join("untracked.txt"), "keep\n")?;
        assert!(ensure_clean_worktree(&linked, &owner).is_err());
        return Ok(());
    }
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("main");
    std::fs::create_dir(&root)?;
    exec_git_cmd(&["init"], &root);
    std::fs::write(root.join(".gitignore"), "ignored.txt\n")?;
    std::fs::write(root.join(".gitattributes"), "* text eol=lf\n")?;
    create_commit_with_message(&root, true, "initial");
    let linked = parent.path().join("linked");
    exec_git_cmd(
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
        &root,
    );
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "operations::repository::tests::worktree_safety_checks_do_not_need_git",
            "--nocapture",
        ])
        .env(FIXTURE, &root)
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
async fn branch_creation_and_tracking_work_without_git_executable() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_BRANCH_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let repo = GitRepository::discover(Path::new(&root))?;
        let before = repo.open_read(true)?.head_id()?.detach();
        execute(
            &repo,
            RepositoryOperation::CreateBranch {
                name: "topic/native".into(),
            },
        )
        .await?;
        let git = repo.open_read(true)?;
        assert_eq!(git.head_id()?.detach(), before);
        assert_eq!(
            git.find_reference("refs/heads/topic/native")?.id().detach(),
            before
        );
        let config = git.config_snapshot();
        assert_eq!(
            config
                .string("branch.topic/native.remote")
                .as_ref()
                .map(|value| value.as_bstr()),
            Some(".".into())
        );
        assert_eq!(
            config
                .string("branch.topic/native.merge")
                .as_ref()
                .map(|value| value.as_bstr()),
            Some("refs/heads/main".into())
        );
        assert_eq!(config.boolean("branch.topic/native.rebase"), Some(true));
        let contents = std::fs::read(repo.common_dir.join("config"))?;
        assert!(execute(
            &repo,
            RepositoryOperation::CreateBranch {
                name: "topic/native".into()
            }
        )
        .await
        .is_err());
        assert_eq!(std::fs::read(repo.common_dir.join("config"))?, contents);
        assert_eq!(
            std::fs::read_to_string(repo.root.join("file.txt"))?,
            "initial\n"
        );
        return Ok(());
    }
    let dir = initial()?;
    exec_git_cmd(&["config", "branch.autoSetupMerge", "always"], dir.path());
    exec_git_cmd(&["config", "branch.autoSetupRebase", "local"], dir.path());
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "operations::repository::tests::branch_creation_and_tracking_work_without_git_executable", "--nocapture"])
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
async fn branch_creation_inherits_tracking_like_git() -> Result<()> {
    let dir = initial()?;
    let root = dir.path();
    exec_git_cmd(&["remote", "add", "origin", "."], root);
    exec_git_cmd(&["config", "branch.main.remote", "origin"], root);
    exec_git_cmd(
        &["config", "branch.main.merge", "refs/heads/upstream"],
        root,
    );
    exec_git_cmd(&["config", "branch.autoSetupMerge", "inherit"], root);
    exec_git_cmd(&["config", "branch.autoSetupRebase", "remote"], root);
    exec_git_cmd(&["branch", "oracle"], root);
    let repo = GitRepository::discover(root)?;
    execute(
        &repo,
        RepositoryOperation::CreateBranch {
            name: "native".into(),
        },
    )
    .await?;
    let git = repo.open_read(true)?;
    let config = git.config_snapshot();
    for key in ["remote", "merge", "rebase"] {
        assert_eq!(
            config.string(&format!("branch.native.{key}")),
            config.string(&format!("branch.oracle.{key}"))
        );
    }
    assert_eq!(
        config
            .string("branch.native.remote")
            .as_ref()
            .map(|value| value.as_bstr()),
        Some("origin".into())
    );
    Ok(())
}

#[test]
fn native_fetch_custom_branch_refspecs_match_git() -> Result<()> {
    type Refs = Vec<(gix::bstr::BString, gix::ObjectId)>;
    fn state(repo: &gix::Repository) -> Result<(Refs, Vec<String>)> {
        let mut refs = Vec::new();
        for reference in repo.references()?.all()? {
            let mut reference = reference?;
            if reference.name().as_bstr().starts_with(b"refs/remotes/") {
                let name = reference.name().as_bstr().to_owned();
                refs.push((name, reference.peel_to_id()?.detach()));
            }
        }
        refs.sort();
        let mut fetch_head: Vec<_> = std::fs::read_to_string(repo.git_dir().join("FETCH_HEAD"))?
            .lines()
            .map(|line| line.split_once(" of ").unwrap().0.to_owned())
            .collect();
        fetch_head.sort();
        Ok((refs, fetch_head))
    }
    let source = initial()?;
    for name in ["other", "release/v1", "private/secret"] {
        exec_git_cmd(&["branch", name], source.path());
    }
    for specs in [
        vec!["refs/heads/main:refs/remotes/origin/trunk"],
        vec!["+refs/heads/release/*:refs/remotes/origin/releases/*"],
        vec!["+refs/heads/*:refs/remotes/origin/*", "^refs/heads/main"],
        vec![
            "+refs/heads/main:refs/remotes/origin/trunk",
            "+refs/heads/release/*:refs/remotes/origin/releases/*",
        ],
        vec![
            "+refs/heads/*:refs/remotes/origin/*",
            "^refs/heads/private/*",
        ],
    ] {
        let native = empty_git_repo();
        let oracle = empty_git_repo();
        for root in [native.path(), oracle.path()] {
            exec_git_cmd(
                &["remote", "add", "origin", source.path().to_str().unwrap()],
                root,
            );
            exec_git_cmd(&["config", "--unset-all", "remote.origin.fetch"], root);
            for spec in &specs {
                exec_git_cmd(&["config", "--add", "remote.origin.fetch", spec], root);
            }
            exec_git_cmd(&["config", "branch.main.remote", "origin"], root);
            exec_git_cmd(&["config", "branch.main.merge", "refs/heads/main"], root);
            for key in [
                "fetch.prune",
                "fetch.pruneTags",
                "fetch.writeCommitGraph",
                "fetch.fsckObjects",
                "transfer.fsckObjects",
            ] {
                exec_git_cmd(&["config", key, "false"], root);
            }
        }
        let repo = GitRepository::discover(native.path())?;
        assert!(
            fetch::execute(&repo, "origin", None)?.is_some(),
            "{specs:?}"
        );
        // Compare tracking refs, default HEAD and FETCH_HEAD with Git.
        exec_git_cmd(
            &["fetch", "--no-recurse-submodules", "origin"],
            oracle.path(),
        );
        let git = repo.open_read(true)?;
        assert_eq!(
            state(&git)?,
            state(&gix::open(oracle.path())?)?,
            "{specs:?}"
        );
        assert!(git.head()?.is_unborn());
        assert!(!native.path().join("file.txt").exists());
        // Pull must preserve the custom refspec policy through its existing fallback.
        assert!(fetch::execute(&repo, "origin", Some("main"))?.is_none());
    }
    Ok(())
}

#[test]
fn native_fetch_defers_refspecs_outside_the_selected_remotes_branches() -> Result<()> {
    let dir = initial()?;
    exec_git_cmd(
        &[
            "remote",
            "add",
            "origin",
            "unavailable-protocol://example.invalid/repo",
        ],
        dir.path(),
    );
    let repo = GitRepository::discover(dir.path())?;
    for spec in [
        "+refs/heads/*:refs/heads/*",
        "+refs/heads/*:refs/remotes/other/*",
        "+refs/tags/*:refs/tags/*",
        "refs/heads/main",
        "^refs/heads/private/*",
    ] {
        exec_git_cmd(&["config", "remote.origin.fetch", spec], dir.path());
        assert!(fetch::execute(&repo, "origin", None)?.is_none(), "{spec}");
        assert!(!repo.git_dir.join("FETCH_HEAD").exists());
    }
    Ok(())
}

#[tokio::test]
async fn native_fetch_updates_tracking_refs_and_fetch_head_without_checkout() -> Result<()> {
    let source = initial()?;
    exec_git_cmd(&["branch", "other"], source.path());
    exec_git_cmd(&["tag", "fixture-v1"], source.path());
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("checkout");
    exec_git_cmd(
        &[
            "clone",
            source.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ],
        directory.path(),
    );
    std::fs::write(source.path().join("file.txt"), "remote change\n")?;
    create_commit_with_message(source.path(), true, "remote change");
    let repository = GitRepository::discover(&root)?;
    let git = repository.open_read(true)?;
    let before = git.head_id()?.detach();
    for key in [
        "fetch.prune",
        "fetch.pruneTags",
        "fetch.writeCommitGraph",
        "fetch.fsckObjects",
        "transfer.fsckObjects",
    ] {
        exec_git_cmd(&["config", key, "false"], &root);
    }
    // Calling the native entry point asserts this configuration doesn't silently fall back.
    assert!(fetch::execute(&repository, "origin", None)?.is_some());
    assert_eq!(git.head_id()?.detach(), before);
    assert_eq!(std::fs::read_to_string(root.join("file.txt"))?, "initial\n");
    let expected = gix::open(source.path())?.head_id()?.detach();
    assert_eq!(
        git.find_reference("refs/remotes/origin/main")?
            .id()
            .detach(),
        expected
    );
    let contents = std::fs::read_to_string(git.git_dir().join("FETCH_HEAD"))?;
    assert!(contents.starts_with(&format!("{expected}\t\tbranch 'main' of origin\n")));
    assert!(contents.contains("\tnot-for-merge\tbranch 'other' of origin\n"));
    assert_eq!(fetched_commit(&git)?.as_str(), expected.to_string());
    let relative = Path::new("..")
        .join("..")
        .join(source.path().file_name().unwrap());
    exec_git_cmd(
        &["remote", "set-url", "origin", relative.to_str().unwrap()],
        &root,
    );
    let fetched = fetch::execute(&repository, "origin", Some("main"))?.unwrap();
    assert_eq!(fetched.target.unwrap().as_str(), expected.to_string());
    execute(&repository, RepositoryOperation::Pull { remote: None }).await?;
    assert_eq!(
        std::fs::read_to_string(root.join("file.txt"))?,
        "remote change\n"
    );
    exec_git_cmd(&["config", "fetch.prune", "true"], &root);
    assert!(fetch::execute(&repository, "origin", None)?.is_none());
    Ok(())
}

#[tokio::test]
async fn branch_pages_replay_and_continue_across_packed_and_loose_refs() -> Result<()> {
    let dir = initial()?;
    let repo = GitRepository::discover(dir.path())?;
    let git = repo.open(false)?;
    let id = git.head_id()?.detach();
    for index in 0..205 {
        git.reference(
            format!("refs/heads/topic-{index:03}"),
            id,
            gix::refs::transaction::PreviousValue::Any,
            "test",
        )?;
    }
    exec_git_cmd(&["pack-refs", "--all"], dir.path());
    git.reference(
        "refs/heads/topic-100",
        id,
        gix::refs::transaction::PreviousValue::Any,
        "loose override",
    )?;
    let cursor = repo.branch_cursor();
    let first = cursor.next_page().await?;
    assert_eq!(first.branches.len(), BRANCH_PAGE_SIZE);
    assert_eq!(cursor.next_page().await?.branches, first.branches);
    let second = first.next.unwrap().next_page().await?;
    assert_eq!(second.branches.len(), BRANCH_PAGE_SIZE);
    let third = second.next.unwrap().next_page().await?;
    assert_eq!(third.branches.len(), 6);
    assert!(third.next.is_none());
    let mut names: Vec<_> = first
        .branches
        .into_iter()
        .chain(second.branches)
        .chain(third.branches)
        .map(|branch| branch.name)
        .collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 206);
    assert_eq!(
        names,
        repo.branches()
            .await?
            .into_iter()
            .map(|branch| branch.name)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[tokio::test]
async fn branch_pages_publish_before_a_later_ref_error() -> Result<()> {
    let dir = initial()?;
    let repo = GitRepository::discover(dir.path())?;
    let git = repo.open(false)?;
    let id = git.head_id()?.detach();
    for index in 0..205 {
        git.reference(
            format!("refs/heads/topic-{index:03}"),
            id,
            gix::refs::transaction::PreviousValue::Any,
            "test",
        )?;
    }
    exec_git_cmd(&["config", "branch.topic-204.remote", "."], dir.path());
    exec_git_cmd(
        &["config", "branch.topic-204.merge", "invalid ref name"],
        dir.path(),
    );
    let first = repo.branch_cursor().next_page().await?;
    assert_eq!(first.branches.len(), BRANCH_PAGE_SIZE);
    let second = first.next.unwrap().next_page().await?;
    assert_eq!(second.branches.len(), BRANCH_PAGE_SIZE);
    assert!(second.next.unwrap().next_page().await.is_err());
    Ok(())
}

#[tokio::test]
async fn branch_pages_report_initialization_errors_and_empty_repositories() -> Result<()> {
    let dir = empty_git_repo();
    let repo = GitRepository::discover(dir.path())?;
    let page = repo.branch_cursor().next_page().await?;
    assert!(page.branches.is_empty());
    assert!(page.next.is_none());
    let cursor = repo.branch_cursor();
    std::fs::rename(dir.path().join(".git"), dir.path().join("old.git"))?;
    let error = cursor.next_page().await.err().unwrap().to_string();
    assert!(error.contains("discover git repo"), "{error}");
    Ok(())
}

#[tokio::test]
async fn branch_deletion_keeps_unmerged_and_checked_out_branches() -> Result<()> {
    let dir = initial()?;
    let repo = GitRepository::discover(dir.path())?;
    execute(
        &repo,
        RepositoryOperation::CreateBranch {
            name: "merged".into(),
        },
    )
    .await?;
    execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "merged".into(),
        },
    )
    .await?;
    assert!(!repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "merged"));
    assert!(execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "main".into()
        }
    )
    .await
    .is_err());
    execute(
        &repo,
        RepositoryOperation::CreateBranch {
            name: "unmerged".into(),
        },
    )
    .await?;
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "unmerged".into(),
        },
    )
    .await?;
    std::fs::write(dir.path().join("file.txt"), "unmerged\n")?;
    create_commit_with_message(dir.path(), true, "unmerged");
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "main".into(),
        },
    )
    .await?;
    assert!(execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "unmerged".into()
        }
    )
    .await
    .is_err());
    execute(
        &repo,
        RepositoryOperation::CreateBranch {
            name: "linked".into(),
        },
    )
    .await?;
    let parent = tempfile::tempdir()?;
    let linked = parent.path().join("linked");
    execute(
        &repo,
        RepositoryOperation::CreateWorktree {
            path: linked,
            branch: Some("linked".into()),
        },
    )
    .await?;
    assert!(execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "linked".into()
        }
    )
    .await
    .is_err());
    for name in ["main", "unmerged", "linked"] {
        assert!(repo
            .branches()
            .await?
            .iter()
            .any(|branch| branch.name == name));
    }
    assert!(repo
        .prepare_repository(
            RepositoryOperation::DeleteBranch {
                name: "--force".into()
            },
            true
        )
        .is_err());
    assert!(repo
        .prepare_repository(
            RepositoryOperation::DeleteBranch {
                name: "unmerged".into()
            },
            false
        )
        .is_err());
    Ok(())
}

#[tokio::test]
async fn native_branch_deletion_removes_packed_refs_and_configuration_without_git() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_BRANCH_DELETE_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let repo = GitRepository::discover(Path::new(&root))?;
        execute(
            &repo,
            RepositoryOperation::DeleteBranch {
                name: "topic/native".into(),
            },
        )
        .await?;
        let git = repo.open_read(true)?;
        assert!(git.try_find_reference("refs/heads/topic/native")?.is_none());
        assert!(git
            .config_snapshot()
            .plumbing()
            .sections_by_name("branch")
            .into_iter()
            .flatten()
            .all(|section| section.header().subsection_name() != Some("topic/native".into())));
        assert!(!repo
            .common_dir
            .join("logs/refs/heads/topic/native")
            .exists());
        assert_eq!(git.head_name()?.unwrap().shorten(), "main");
        return Ok(());
    }
    let dir = initial()?;
    exec_git_cmd(&["branch", "topic/native"], dir.path());
    exec_git_cmd(&["config", "branch.topic/native.remote", "."], dir.path());
    exec_git_cmd(
        &["config", "branch.topic/native.merge", "refs/heads/main"],
        dir.path(),
    );
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join(".git/config"))?
        .write_all(b"\n[branch \"topic/native\"]\n\tdescription = remove this too\n")?;
    exec_git_cmd(&["pack-refs", "--all"], dir.path());
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "operations::repository::tests::native_branch_deletion_removes_packed_refs_and_configuration_without_git", "--nocapture"])
        .env(FIXTURE, dir.path()).env("PATH", no_programs.path()).env("GIT_EXEC_PATH", no_programs.path()).output()?;
    ensure!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    Ok(())
}

#[tokio::test]
async fn native_branch_deletion_uses_upstream_merge_state_and_preserves_configuration_on_failure(
) -> Result<()> {
    let dir = initial()?;
    let repo = GitRepository::discover(dir.path())?;
    exec_git_cmd(&["checkout", "-b", "topic"], dir.path());
    std::fs::write(dir.path().join("file.txt"), "topic\n")?;
    create_commit_with_message(dir.path(), true, "topic");
    exec_git_cmd(&["branch", "upstream"], dir.path());
    exec_git_cmd(&["checkout", "main"], dir.path());
    exec_git_cmd(&["config", "branch.topic.remote", "."], dir.path());
    exec_git_cmd(
        &["config", "branch.topic.merge", "refs/heads/main"],
        dir.path(),
    );
    let config = std::fs::read(repo.common_dir.join("config"))?;
    assert!(execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "topic".into()
        }
    )
    .await
    .is_err());
    assert_eq!(std::fs::read(repo.common_dir.join("config"))?, config);
    // Match git branch -d: the upstream, when it exists, takes precedence over HEAD.
    exec_git_cmd(
        &["config", "branch.topic.merge", "refs/heads/upstream"],
        dir.path(),
    );
    let lock = repo.common_dir.join("config.lock");
    std::fs::write(&lock, "locked")?;
    assert!(execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "topic".into()
        }
    )
    .await
    .is_err());
    assert!(repo
        .open_read(true)?
        .try_find_reference("refs/heads/topic")?
        .is_some());
    std::fs::remove_file(lock)?;
    execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "topic".into(),
        },
    )
    .await?;
    assert!(!repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic"));
    // A branch merged into HEAD but not into its upstream must also be kept.
    exec_git_cmd(&["checkout", "upstream"], dir.path());
    exec_git_cmd(&["branch", "keep"], dir.path());
    exec_git_cmd(&["config", "branch.keep.remote", "."], dir.path());
    exec_git_cmd(
        &["config", "branch.keep.merge", "refs/heads/main"],
        dir.path(),
    );
    assert!(execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "keep".into()
        }
    )
    .await
    .is_err());
    exec_git_cmd(
        &["config", "branch.keep.merge", "refs/heads/missing"],
        dir.path(),
    );
    execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "keep".into(),
        },
    )
    .await?;
    exec_git_cmd(
        &["symbolic-ref", "refs/heads/alias", "refs/heads/main"],
        dir.path(),
    );
    assert!(execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "alias".into()
        }
    )
    .await
    .is_err());
    assert!(repo
        .open_read(true)?
        .try_find_reference("refs/heads/main")?
        .is_some());
    Ok(())
}

#[tokio::test]
async fn native_branch_deletion_keeps_branches_reserved_by_detached_worktrees() -> Result<()> {
    let dir = initial()?;
    let repo = GitRepository::discover(dir.path())?;
    exec_git_cmd(&["branch", "keep"], dir.path());
    let parent = tempfile::tempdir()?;
    let linked = parent.path().join("linked");
    execute(
        &repo,
        RepositoryOperation::CreateWorktree {
            path: linked.clone(),
            branch: None,
        },
    )
    .await?;
    let git = GitRepository::discover(&linked)?.git_dir;
    for (file, contents) in [
        ("rebase-merge/head-name", "refs/heads/keep\n"),
        ("rebase-apply/head-name", "refs/heads/keep\n"),
        ("BISECT_START", "keep\n"),
        ("rebase-merge/update-refs", "refs/heads/keep\n0000000000000000000000000000000000000000\n0000000000000000000000000000000000000000\n"),
    ] {
        let path = git.join(file);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, contents)?;
        if file == "BISECT_START" { std::fs::write(git.join("BISECT_LOG"), "bisect started\n")?; }
        assert!(execute(&repo, RepositoryOperation::DeleteBranch { name: "keep".into() }).await.is_err(), "{file}");
        assert!(execute(&repo, RepositoryOperation::SwitchBranch { name: "keep".into() }).await.is_err(), "{file}");
        let destination = parent.path().join("reserved");
        assert!(execute(&repo, RepositoryOperation::CreateWorktree { path: destination.clone(), branch: Some("keep".into()) }).await.is_err(), "{file}");
        assert!(!destination.exists());
        assert!(repo.open_read(true)?.try_find_reference("refs/heads/keep")?.is_some());
        std::fs::remove_file(&path)?;
        if file == "BISECT_START" { std::fs::remove_file(git.join("BISECT_LOG"))?; }
    }
    execute(
        &repo,
        RepositoryOperation::DeleteBranch {
            name: "keep".into(),
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn branch_and_worktree_actions_work_without_git_executable() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_LIFECYCLE_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let repo = GitRepository::discover(Path::new(&root))?;
        std::fs::write(repo.root.join("file.txt"), "local work\n")?;
        execute(
            &repo,
            RepositoryOperation::SwitchBranch {
                name: "target".into(),
            },
        )
        .await?;
        assert_eq!(
            std::fs::read_to_string(repo.root.join("branch.txt"))?,
            "target\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.root.join("file.txt"))?,
            "local work\n"
        );
        execute(
            &repo,
            RepositoryOperation::SwitchBranch {
                name: "main".into(),
            },
        )
        .await?;
        assert!(!repo.root.join("branch.txt").exists());
        execute(
            &repo,
            RepositoryOperation::CreateBranch {
                name: "native".into(),
            },
        )
        .await?;
        let linked = repo.root.parent().unwrap().join("native-worktree");
        execute(
            &repo,
            RepositoryOperation::CreateWorktree {
                path: linked.clone(),
                branch: Some("native".into()),
            },
        )
        .await?;
        let linked_repo = GitRepository::discover(&linked)?;
        assert_eq!(
            linked_repo.open_read(true)?.head_name()?.unwrap().shorten(),
            "native"
        );
        std::fs::write(linked.join("untracked.txt"), "keep\n")?;
        assert!(execute(
            &repo,
            RepositoryOperation::RemoveWorktree {
                path: linked.clone()
            }
        )
        .await
        .is_err());
        std::fs::remove_file(linked.join("untracked.txt"))?;
        execute(&repo, RepositoryOperation::RemoveWorktree { path: linked }).await?;
        execute(
            &repo,
            RepositoryOperation::DeleteBranch {
                name: "native".into(),
            },
        )
        .await?;
        assert_eq!(
            std::fs::read_to_string(repo.root.join("file.txt"))?,
            "local work\n"
        );
        return Ok(());
    }
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("repo");
    let initial = initial()?;
    std::fs::rename(initial.path(), &root)?;
    exec_git_cmd(&["switch", "-c", "target"], &root);
    std::fs::write(root.join("branch.txt"), "target\n")?;
    create_commit_with_message(&root, true, "target");
    exec_git_cmd(&["switch", "main"], &root);
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "operations::repository::tests::branch_and_worktree_actions_work_without_git_executable", "--nocapture"])
        .env(FIXTURE, root).env("PATH", no_programs.path()).env("GIT_EXEC_PATH", no_programs.path()).output()?;
    ensure!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    Ok(())
}

#[tokio::test]
async fn native_switch_preserves_unrelated_staged_edits_and_rejects_overwrites_before_writing(
) -> Result<()> {
    let dir = initial()?;
    std::fs::write(dir.path().join("local.txt"), "base\n")?;
    std::fs::write(dir.path().join("obsolete.txt"), "obsolete\n")?;
    create_commit_with_message(dir.path(), true, "base");
    exec_git_cmd(&["switch", "-c", "target"], dir.path());
    std::fs::write(dir.path().join("file.txt"), "target\n")?;
    std::fs::remove_file(dir.path().join("obsolete.txt"))?;
    std::fs::write(dir.path().join("new.txt"), "new\n")?;
    create_commit_with_message(dir.path(), true, "target");
    exec_git_cmd(&["switch", "main"], dir.path());
    let repo = GitRepository::discover(dir.path())?;
    std::fs::write(dir.path().join("file.txt"), "precious\n")?;
    let original_index = std::fs::read(repo.git_dir.join("index"))?;
    assert!(execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "target".into()
        }
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "precious\n"
    );
    assert_eq!(std::fs::read(repo.git_dir.join("index"))?, original_index);
    assert!(!dir.path().join("new.txt").exists());
    std::fs::write(dir.path().join("file.txt"), "initial\n")?;
    std::fs::write(dir.path().join("local.txt"), "staged\n")?;
    exec_git_cmd(&["add", "local.txt"], dir.path());
    let staged_id = repo
        .open_read(true)?
        .index()?
        .entry_by_path("local.txt".into())
        .unwrap()
        .id;
    std::fs::write(dir.path().join("local.txt"), "unstaged\n")?;
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "target".into(),
        },
    )
    .await?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "target\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("local.txt"))?,
        "unstaged\n"
    );
    assert_eq!(
        repo.open_read(true)?
            .index()?
            .entry_by_path("local.txt".into())
            .unwrap()
            .id,
        staged_id
    );
    assert!(!dir.path().join("obsolete.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("new.txt"))?,
        "new\n"
    );
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "main".into(),
        },
    )
    .await?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "initial\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("obsolete.txt"))?,
        "obsolete\n"
    );
    Ok(())
}

#[tokio::test]
async fn native_switch_handles_file_directory_transitions_without_removing_untracked_files(
) -> Result<()> {
    let dir = initial()?;
    std::fs::write(dir.path().join("node"), "file\n")?;
    create_commit_with_message(dir.path(), true, "file");
    exec_git_cmd(&["switch", "-c", "directory"], dir.path());
    std::fs::remove_file(dir.path().join("node"))?;
    std::fs::create_dir(dir.path().join("node"))?;
    std::fs::write(dir.path().join("node/leaf"), "leaf\n")?;
    create_commit_with_message(dir.path(), true, "directory");
    exec_git_cmd(&["switch", "main"], dir.path());
    let repo = GitRepository::discover(dir.path())?;
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "directory".into(),
        },
    )
    .await?;
    std::fs::write(dir.path().join("node/untracked"), "precious\n")?;
    assert!(execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "main".into()
        }
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("node/untracked"))?,
        "precious\n"
    );
    std::fs::remove_file(dir.path().join("node/untracked"))?;
    execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "main".into(),
        },
    )
    .await?;
    assert_eq!(std::fs::read_to_string(dir.path().join("node"))?, "file\n");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn native_switch_prepares_required_filters_before_changing_files_or_head() -> Result<()> {
    let dir = initial()?;
    exec_git_cmd(&["switch", "-c", "filtered"], dir.path());
    std::fs::write(dir.path().join(".gitattributes"), "*.txt filter=reject\n")?;
    std::fs::write(dir.path().join("file.txt"), "filtered\n")?;
    std::fs::write(dir.path().join("new.txt"), "new\n")?;
    create_commit_with_message(dir.path(), true, "filtered");
    exec_git_cmd(&["switch", "main"], dir.path());
    exec_git_cmd(&["config", "filter.reject.smudge", "exit 1"], dir.path());
    exec_git_cmd(&["config", "filter.reject.required", "true"], dir.path());
    let repo = GitRepository::discover(dir.path())?;
    let head = repo.open_read(true)?.head_id()?.detach();
    let index = std::fs::read(repo.git_dir.join("index"))?;
    assert!(execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "filtered".into()
        }
    )
    .await
    .is_err());
    assert_eq!(repo.open_read(true)?.head_id()?.detach(), head);
    assert_eq!(std::fs::read(repo.git_dir.join("index"))?, index);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "initial\n"
    );
    let smudge = format!(
        "printf raced > '{}'; cat",
        dir.path().join("file.txt").display()
    );
    exec_git_cmd(&["config", "filter.reject.smudge", &smudge], dir.path());
    assert!(execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "filtered".into()
        }
    )
    .await
    .is_err());
    assert_eq!(repo.open_read(true)?.head_id()?.detach(), head);
    assert_eq!(std::fs::read(repo.git_dir.join("index"))?, index);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "raced"
    );
    std::fs::write(dir.path().join("file.txt"), "initial\n")?;
    let smudge = format!(
        "printf external > '{}'; cat",
        dir.path().join("new.txt").display()
    );
    exec_git_cmd(&["config", "filter.reject.smudge", &smudge], dir.path());
    assert!(execute(
        &repo,
        RepositoryOperation::SwitchBranch {
            name: "filtered".into()
        }
    )
    .await
    .is_err());
    assert_eq!(repo.open_read(true)?.head_id()?.detach(), head);
    assert_eq!(std::fs::read(repo.git_dir.join("index"))?, index);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "initial\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("new.txt"))?,
        "external"
    );
    Ok(())
}

#[tokio::test]
async fn linked_worktrees_can_be_created_without_a_configured_identity() -> Result<()> {
    const CHILD: &str = "MITOS_TEST_WORKTREE_WITHOUT_IDENTITY";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir()?;
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "operations::repository::tests::linked_worktrees_can_be_created_without_a_configured_identity",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", home.path().join(".gitconfig"))
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL")
            .env_remove("EMAIL")
            .output()?;
        ensure!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    let dir = initial()?;
    exec_git_cmd(&["config", "--unset", "user.name"], dir.path());
    exec_git_cmd(&["config", "--unset", "user.email"], dir.path());
    let repo = GitRepository::discover(dir.path())?;
    assert!(repo.open_read(true)?.committer().is_none());
    let parent = tempfile::tempdir()?;
    for (name, branch) in [("detached", None), ("attached", Some("main".into()))] {
        if branch.is_some() {
            exec_git_cmd(&["checkout", "--detach"], dir.path());
        }
        let linked = parent.path().join(name);
        execute(
            &repo,
            RepositoryOperation::CreateWorktree {
                path: linked.clone(),
                branch,
            },
        )
        .await?;
        assert_eq!(
            std::fs::read_to_string(linked.join("file.txt"))?,
            "initial\n"
        );
        let linked_repo = GitRepository::discover(&linked)?;
        let log = std::fs::read_to_string(linked_repo.git_dir.join("logs/HEAD"))?;
        assert!(
            log.contains("no name configured <noEmailAvailable@example.com>"),
            "{log}"
        );
    }
    Ok(())
}

#[test]
fn native_fetch_records_default_head_like_git() -> Result<()> {
    for protocol in ["1", "2"] {
        for case in [
            "missing", "create", "never", "symbolic", "direct", "custom", "excluded", "detached",
            "unborn",
        ] {
            let source = initial()?;
            exec_git_cmd(&["branch", "other"], source.path());
            match case {
                "detached" => exec_git_cmd(&["checkout", "--detach"], source.path()),
                "unborn" => exec_git_cmd(
                    &["symbolic-ref", "HEAD", "refs/heads/missing"],
                    source.path(),
                ),
                _ => {}
            }
            let native = empty_git_repo();
            let oracle = empty_git_repo();
            for root in [native.path(), oracle.path()] {
                exec_git_cmd(
                    &["remote", "add", "origin", source.path().to_str().unwrap()],
                    root,
                );
                exec_git_cmd(&["config", "protocol.version", protocol], root);
                for key in [
                    "fetch.prune",
                    "fetch.pruneTags",
                    "fetch.writeCommitGraph",
                    "fetch.fsckObjects",
                    "transfer.fsckObjects",
                ] {
                    exec_git_cmd(&["config", key, "false"], root);
                }
                match case {
                    "create" | "never" => {
                        exec_git_cmd(&["config", "remote.origin.followRemoteHEAD", case], root)
                    }
                    "symbolic" => exec_git_cmd(
                        &[
                            "symbolic-ref",
                            "refs/remotes/origin/HEAD",
                            "refs/remotes/origin/other",
                        ],
                        root,
                    ),
                    "direct" => {
                        exec_git_cmd(&["fetch", "origin"], root);
                        let id = gix::open(source.path())?.head_id()?.to_string();
                        exec_git_cmd(
                            &["update-ref", "--no-deref", "refs/remotes/origin/HEAD", &id],
                            root,
                        );
                    }
                    "custom" => exec_git_cmd(
                        &[
                            "config",
                            "remote.origin.fetch",
                            "+refs/heads/*:refs/remotes/origin/branches/*",
                        ],
                        root,
                    ),
                    "excluded" => exec_git_cmd(
                        &["config", "--add", "remote.origin.fetch", "^refs/heads/main"],
                        root,
                    ),
                    _ => {}
                }
            }
            let repository = GitRepository::discover(native.path())?;
            assert!(
                fetch::execute(&repository, "origin", None)?.is_some(),
                "{case} protocol {protocol}"
            );
            exec_git_cmd(
                &["fetch", "--no-recurse-submodules", "origin"],
                oracle.path(),
            );
            let head = |root: &Path| -> Result<_> {
                let repo = gix::open(root)?;
                Ok(repo
                    .try_find_reference("refs/remotes/origin/HEAD")?
                    .map(|reference| reference.target().into_owned()))
            };
            assert_eq!(
                head(native.path())?,
                head(oracle.path())?,
                "{case} protocol {protocol}"
            );
            // The advertised HEAD is inspection metadata and never an extra fetched entry.
            assert!(
                !std::fs::read_to_string(repository.git_dir.join("FETCH_HEAD"))?
                    .contains("ref 'HEAD'")
            );
            if matches!(case, "missing" | "create") {
                let git = repository.open_read(true)?;
                let remote = git.find_remote("origin")?;
                assert_eq!(
                    remote.default_branch()?.unwrap().as_bstr(),
                    b"refs/heads/main".as_bstr()
                );
            }
        }
    }
    Ok(())
}

#[test]
fn native_fetch_defers_unsupported_remote_head_policies_before_network() -> Result<()> {
    let dir = initial()?;
    exec_git_cmd(
        &[
            "remote",
            "add",
            "origin",
            "unavailable-protocol://example.invalid/repo",
        ],
        dir.path(),
    );
    for key in [
        "fetch.prune",
        "fetch.pruneTags",
        "fetch.writeCommitGraph",
        "fetch.fsckObjects",
        "transfer.fsckObjects",
    ] {
        exec_git_cmd(&["config", key, "false"], dir.path());
    }
    let repository = GitRepository::discover(dir.path())?;
    for policy in ["always", "warn", "warn-if-not-main", "invalid"] {
        exec_git_cmd(
            &["config", "remote.origin.followRemoteHEAD", policy],
            dir.path(),
        );
        assert!(fetch::execute(&repository, "origin", None)?.is_none());
        assert!(!repository.git_dir.join("FETCH_HEAD").exists());
    }
    Ok(())
}
