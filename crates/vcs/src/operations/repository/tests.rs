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
