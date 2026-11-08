use super::*;
use crate::git::test::{create_commit_with_message, empty_git_repo, exec_git_cmd};

fn commit(root: &Path, text: &str) {
    create_commit_with_message(root, true, text);
}
fn current(repo: &GitRepository) -> CommitEntry {
    let git = repo.open(false).unwrap();
    let commit = git.head_commit().unwrap();
    CommitEntry {
        id: RevisionId::parse(&commit.id.to_string()).unwrap(),
        parents: commit
            .parent_ids()
            .map(|id| RevisionId::parse(&id.to_string()).unwrap())
            .collect(),
        author: String::new(),
        date: String::new(),
        subject: String::new(),
        file: None,
    }
}

#[tokio::test]
async fn full_commit_messages_are_loaded_from_the_pinned_revision() -> Result<()> {
    let dir = empty_git_repo();
    let body = format!(
        "Full body with 日本語.\n\n{}\nmessage-tail",
        "body\n".repeat(400)
    );
    let message = format!("Subject\n\n{body}");
    std::fs::write(dir.path().join("history.txt"), "initial\n")?;
    commit(dir.path(), &message);
    let repo = GitRepository::discover(dir.path())?;
    let selected = current(&repo).id;
    std::fs::write(dir.path().join("history.txt"), "later\n")?;
    commit(dir.path(), "Later commit");
    assert_eq!(repo.commit_message(&selected).await?.trim_end(), message);
    assert!(repo
        .commit_message(&RevisionId::parse(&"0".repeat(40))?)
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn history_pins_objects_follows_renames_and_handles_root_and_deletion() -> Result<()> {
    let dir = empty_git_repo();
    let root = dir.path();
    std::fs::write(root.join(".gitattributes"), "* text eol=lf\n")?;
    let old = root.join("old [name].txt");
    let new = root.join("new [name].txt");
    std::fs::write(&old, "original\n")?;
    commit(root, "initial");
    let repo = GitRepository::discover(root)?;
    let initial = current(&repo);
    exec_git_cmd(&["mv", "old [name].txt", "new [name].txt"], root);
    commit(root, "renamed");
    let pinned = repo.history_cursor(Some(&new))?.unwrap();
    std::fs::write(&new, "changed\n")?;
    commit(root, "later");
    let page = repo.history_page(pinned).await?;
    assert_eq!(page.commits.len(), 2);
    assert_eq!(page.commits[0].subject, "renamed");
    assert_eq!(page.commits[1].id, initial.id);
    let rename = page.commits[0].file.as_ref().unwrap();
    assert_eq!(rename.before.as_deref(), Some(Path::new("old [name].txt")));
    assert_eq!(rename.after.as_deref(), Some(Path::new("new [name].txt")));
    let (before, after) = rename.endpoints(&page.commits[0], 0)?;
    assert_eq!(
        repo.read_revision_file(before.as_ref().unwrap(), false)?,
        b"original\n"
    );
    assert_eq!(
        repo.read_revision_file(after.as_ref().unwrap(), false)?,
        b"original\n"
    );
    let root_files = repo.commit_files(&initial, 0).await?;
    let root_file = root_files
        .iter()
        .find(|file| file.path() == Path::new("old [name].txt"))
        .unwrap();
    assert!(root_file.endpoints(&initial, 0)?.0.is_none());
    std::fs::remove_file(&new)?;
    commit(root, "deleted");
    let deleted = current(&repo);
    let files = repo.commit_files(&deleted, 0).await?;
    assert!(files[0].after.is_none());
    assert_eq!(
        repo.read_revision_file(files[0].endpoints(&deleted, 0)?.0.as_ref().unwrap(), false)?,
        b"changed\n"
    );
    let page = repo
        .history_page(repo.history_cursor(Some(&new))?.unwrap())
        .await?;
    assert_eq!(page.commits.len(), 4);
    assert!(page.commits[0].file.as_ref().unwrap().after.is_none());
    assert!(RevisionId::parse("HEAD").is_err());
    assert!(RevisionFile::new(initial.id, PathBuf::from("../outside")).is_err());
    Ok(())
}

#[tokio::test]
async fn continuation_is_pinned_and_tracks_a_rename_across_pages() -> Result<()> {
    let dir = empty_git_repo();
    let root = dir.path();
    std::fs::write(root.join(".gitattributes"), "* text eol=lf\n")?;
    std::fs::write(root.join("old.txt"), "0\n")?;
    commit(root, "root");
    let repo = GitRepository::discover(root)?;
    for i in 1..=102 {
        if i == 3 {
            exec_git_cmd(&["mv", "old.txt", "new.txt"], root);
        }
        let path = if i >= 3 { "new.txt" } else { "old.txt" };
        // Keep the rename unchanged so Git's heuristic can follow it exactly.
        if i != 3 {
            std::fs::write(root.join(path), format!("{i}\n"))?;
        }
        commit(root, &format!("commit {i}"));
    }
    let first = repo
        .history_page(repo.history_cursor(Some(&root.join("new.txt")))?.unwrap())
        .await?;
    assert_eq!(first.commits.len(), HISTORY_PAGE_SIZE);
    let repository_page = repo
        .history_page(repo.history_cursor(None)?.unwrap())
        .await?;
    std::fs::write(root.join("new.txt"), "new branch tip\n")?;
    commit(root, "after browser opened");
    let second = repo.history_page(first.next.unwrap()).await?;
    assert_eq!(second.commits.len(), 3);
    assert_eq!(second.commits[0].subject, "commit 2");
    assert_eq!(
        second.commits[0].file.as_ref().unwrap().path(),
        Path::new("old.txt")
    );
    assert!(second.next.is_none());
    let repository_older = repo.history_page(repository_page.next.unwrap()).await?;
    assert_eq!(repository_older.commits.len(), 3);
    assert_eq!(repository_older.commits[0].subject, "commit 2");
    Ok(())
}

#[tokio::test]
async fn merge_files_use_the_explicit_parent_and_missing_objects_report_errors() -> Result<()> {
    let dir = empty_git_repo();
    let root = dir.path();
    std::fs::write(root.join("file.txt"), "base\n")?;
    commit(root, "root");
    let repo = GitRepository::discover(root)?;
    exec_git_cmd(&["checkout", "-b", "side"], root);
    std::fs::write(root.join("side.txt"), "side\n")?;
    commit(root, "side");
    exec_git_cmd(&["checkout", "-"], root);
    std::fs::write(root.join("main.txt"), "main\n")?;
    commit(root, "main");
    exec_git_cmd(
        &[
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--no-ff",
            "side",
            "-m",
            "merge",
        ],
        root,
    );
    let merge = current(&repo);
    assert_eq!(merge.parents.len(), 2);
    assert_eq!(
        repo.commit_files(&merge, 0).await?[0].path(),
        Path::new("side.txt")
    );
    assert_eq!(
        repo.commit_files(&merge, 1).await?[0].path(),
        Path::new("main.txt")
    );
    assert!(repo.commit_files(&merge, 2).await.is_err());
    let missing = RevisionFile::new(RevisionId::parse(&"0".repeat(40))?, "file.txt".into())?;
    assert!(repo
        .read_revision_file(&missing, false)
        .unwrap_err()
        .to_string()
        .contains("unavailable"));
    let page = repo
        .history_page(repo.history_cursor(None)?.unwrap())
        .await?;
    assert_eq!(page.commits[0].id, merge.id);
    assert_eq!(
        page.commits.len(),
        4,
        "repository history includes commits from merged branches"
    );
    Ok(())
}

#[tokio::test]
async fn repository_history_uses_git_default_order_across_merged_branches() -> Result<()> {
    let dir = empty_git_repo();
    std::fs::write(dir.path().join("file.txt"), "root\n")?;
    commit(dir.path(), "root");
    let repo = GitRepository::discover(dir.path())?;
    let root = current(&repo).id;
    let tree = repo.open(false)?.head_commit()?.tree_id()?.to_string();
    async fn create(
        repo: &GitRepository,
        tree: &str,
        parents: &[&RevisionId],
        time: u64,
        subject: &str,
    ) -> Result<RevisionId> {
        let mut cmd = repo.command();
        cmd.args(["commit-tree", tree, "-m", subject])
            .env("GIT_AUTHOR_NAME", "History Test")
            .env("GIT_AUTHOR_EMAIL", "history@example.com")
            .env("GIT_COMMITTER_NAME", "History Test")
            .env("GIT_COMMITTER_EMAIL", "history@example.com")
            .env("GIT_AUTHOR_DATE", format!("{time} +0000"))
            .env("GIT_COMMITTER_DATE", format!("{time} +0000"));
        for parent in parents {
            cmd.args(["-p", parent.as_str()]);
        }
        let id = run(cmd).await?;
        RevisionId::parse(std::str::from_utf8(&id)?.trim())
    }
    let side_old = create(&repo, &tree, &[&root], 1_600_000_100, "side old").await?;
    let main = create(&repo, &tree, &[&root], 1_600_000_200, "main").await?;
    let side_new = create(&repo, &tree, &[&side_old], 1_600_000_300, "side new").await?;
    let merge = create(&repo, &tree, &[&main, &side_new], 1_600_000_400, "merge").await?;
    let mut update = repo.command();
    update.args(["update-ref", "HEAD", merge.as_str()]);
    run(update).await?;
    let cursor = repo.history_cursor(None)?.unwrap();
    let page = repo.history_page(cursor).await?;
    let subjects: Vec<_> = page
        .commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(subjects, ["merge", "side new", "main", "side old", "root"]);
    assert!(page.next.is_none());

    Ok(())
}

#[tokio::test]
async fn history_reads_work_without_git_executable() -> Result<()> {
    const FIXTURE: &str = "MITOS_NATIVE_HISTORY_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE) {
        let root = PathBuf::from(root);
        let repo = GitRepository::discover(&root)?;
        let cursor = repo
            .history_cursor(Some(&root.join("new name.txt")))?
            .unwrap();
        let first = repo.history_page(cursor.clone()).await?;
        assert_eq!(first.commits.len(), 2);
        assert!(first.next.is_none());
        assert_eq!(first.commits[0].subject, "renamed and edited");
        assert_eq!(first.commits[0].date, "2024-01-02T10:30:00+00:00");
        let renamed = first.commits[0].file.as_ref().unwrap();
        assert_eq!(renamed.before.as_deref(), Some(Path::new("old name.txt")));
        assert_eq!(renamed.after.as_deref(), Some(Path::new("new name.txt")));
        assert!(renamed.status.starts_with('R'));
        let files = repo.commit_files(&first.commits[0], 0).await?;
        assert!(files.contains(renamed));
        let current = RevisionFile::new(first.commits[0].id.clone(), "new name.txt".into())?;
        assert!(repo
            .read_revision_file(&current, false)?
            .ends_with(b"edited\n"));
        let repeated = repo.history_page(cursor).await?;
        assert!(std::sync::Arc::ptr_eq(
            &first.commits[0],
            &repeated.commits[0]
        ));
        assert_eq!(
            first.commits, repeated.commits,
            "cursor retries must be stable"
        );
        assert_eq!(
            repo.history_page(repo.history_cursor(None)?.unwrap())
                .await?
                .commits
                .len(),
            2
        );
        return Ok(());
    }
    let dir = empty_git_repo();
    let root = dir.path();
    let original: String = (0..20).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("old name.txt"), &original)?;
    std::fs::write(root.join(".gitattributes"), "*.txt text eol=lf\n")?;
    commit(root, "root");
    exec_git_cmd(&["mv", "old name.txt", "new name.txt"], root);
    std::fs::write(root.join("new name.txt"), original + "edited\n")?;
    exec_git_cmd(&["add", "."], root);
    let committed = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "renamed and edited",
        ])
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_DATE", "2020-01-01T23:30:00-1100")
        .env("GIT_COMMITTER_DATE", "2024-01-01T23:30:00-1100")
        .output()?;
    ensure!(
        committed.status.success(),
        "{}",
        String::from_utf8_lossy(&committed.stderr)
    );
    let no_programs = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "operations::history::tests::history_reads_work_without_git_executable",
            "--nocapture",
        ])
        .env(FIXTURE, root)
        .env("PATH", no_programs.path())
        .env("GIT_EXEC_PATH", no_programs.path())
        .output()?;
    ensure!(
        child.status.success(),
        "native history without Git failed:\n{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    Ok(())
}
