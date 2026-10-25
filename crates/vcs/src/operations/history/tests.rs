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
        3,
        "repository history explicitly follows first parents"
    );
    Ok(())
}
