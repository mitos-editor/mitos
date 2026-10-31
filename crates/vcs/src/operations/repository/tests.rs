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
    assert_eq!(repo.remote_names()?, vec!["broken"]);
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
