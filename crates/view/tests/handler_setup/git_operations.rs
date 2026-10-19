use super::Fixture;
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use vcs::{operations::FileOperation, FileChange, FileStatus};
use view::{
    current_ref,
    features::git_operations::{GitAction, GitRequest},
};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn fixture() -> anyhow::Result<Fixture> {
    let f = Fixture::new("head\n")?;
    git(f.dir.path(), &["init"]);
    git(f.dir.path(), &["config", "core.autocrlf", "false"]);
    git(f.dir.path(), &["config", "user.name", "test"]);
    git(f.dir.path(), &["config", "user.email", "test@example.com"]);
    git(f.dir.path(), &["add", "."]);
    git(f.dir.path(), &["commit", "-m", "initial"]);
    Ok(f)
}
fn stash(f: &Fixture) -> anyhow::Result<GitRequest> {
    GitRequest::capture(
        &f.editor,
        f.dir.path().into(),
        GitAction::Stash(f.dir.path().into()),
    )
}
fn path(f: &Fixture) -> PathBuf {
    current_ref!(f.editor).1.path().unwrap().into()
}

#[tokio::test(flavor = "multi_thread")]
async fn stash_rechecks_dirty_buffers_after_preflight_and_releases_admission() -> anyhow::Result<()>
{
    let mut f = fixture()?;
    let request = stash(&f)?;
    assert!(stash(&f).is_err());
    let prepared = request.prepare().await?;
    f.replace("unsaved\n");
    assert!(prepared.admit(&mut f.editor).is_err());
    assert!(!f.editor.handlers().git_operations.busy());
    assert_eq!(std::fs::read_to_string(path(&f))?, "head\n");
    assert_eq!(current_ref!(f.editor).1.text().to_string(), "unsaved\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stash_blocks_force_saves_and_reloads_clean_buffers_without_watchers() -> anyhow::Result<()>
{
    let mut f = fixture()?;
    let path = path(&f);
    let id = current_ref!(f.editor).1.id();
    std::fs::write(&path, "disk\n")?;
    f.editor.reload_document(id)?;
    let task = stash(&f)?.prepare().await?.admit(&mut f.editor)?;
    assert!(f.editor.save(id, None::<PathBuf>, true).is_err());
    assert!(f
        .editor
        .save(id, Some(f.dir.path().join("new-file")), true)
        .is_err());
    task.execute().await.finish(&mut f.editor);
    assert_eq!(current_ref!(f.editor).1.text().to_string(), "head\n");
    assert!(!current_ref!(f.editor).1.is_modified());
    assert!(!f.editor.file_writes_blocked(&path));
    assert!(!f.editor.handlers().git_operations.busy());
    assert_eq!(f.editor.handlers().git_operations.revision(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicted_apply_reloads_markers_and_keeps_the_stash() -> anyhow::Result<()> {
    use vcs::operations::GitRepository;
    let mut f = fixture()?;
    let path = path(&f);
    let id = current_ref!(f.editor).1.id();
    let repo = GitRepository::discover(f.dir.path())?;
    std::fs::write(&path, "stashed\n")?;
    repo.prepare_stash(true)?.execute().await?;
    let entries = repo.stashes().await?;
    std::fs::write(&path, "later commit\n")?;
    git(f.dir.path(), &["add", "."]);
    git(f.dir.path(), &["commit", "-m", "later"]);
    f.editor.reload_document(id)?;
    let request = GitRequest::capture(
        &f.editor,
        f.dir.path().into(),
        GitAction::Apply(repo.clone(), entries[0].clone(), false),
    )?;
    request
        .prepare()
        .await?
        .admit(&mut f.editor)?
        .execute()
        .await
        .finish(&mut f.editor);
    assert!(current_ref!(f.editor)
        .1
        .text()
        .to_string()
        .contains("<<<<<<<"));
    assert!(!f.editor.file_writes_blocked(&path));
    assert_eq!(repo.stashes().await?, entries);
    assert_eq!(f.editor.handlers().git_operations.revision(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn admission_rechecks_workspace_trust_and_editor_ownership() -> anyhow::Result<()> {
    let mut f = fixture()?;
    let mut other = fixture()?;
    let prepared = stash(&f)?.prepare().await?;
    assert!(prepared.admit(&mut other.editor).is_err());
    assert!(!f.editor.handlers().git_operations.busy());
    let prepared = stash(&f)?.prepare().await?;
    f.editor
        .workspace_trust
        .set_config(workspace::trust::Config::default());
    assert!(prepared.admit(&mut f.editor).is_err());
    assert!(!f.editor.handlers().git_operations.busy());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn clean_untracked_buffers_opened_during_stash_are_closed_after_the_file_is_saved_in_git(
) -> anyhow::Result<()> {
    use vcs::operations::GitRepository;
    let mut f = fixture()?;
    let path = f.dir.path().join("untracked.txt");
    std::fs::write(&path, "untracked\n")?;
    let task = stash(&f)?.prepare().await?.admit(&mut f.editor)?;
    let id = f.editor.open(&path, view::editor::Action::Replace)?;
    task.execute().await.finish(&mut f.editor);
    assert!(!path.exists());
    assert!(f.editor.document(id).is_none());
    let repo = GitRepository::discover(f.dir.path())?;
    let entry = repo.stashes().await?.remove(0);
    let files = repo.stash_files(&entry).await?;
    assert_eq!(
        repo.read_stash_file(&entry, &files[0], false)?.after,
        b"untracked\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_during_stash_are_preserved_and_can_be_saved_after_completion() -> anyhow::Result<()>
{
    let mut f = fixture()?;
    let path = path(&f);
    let id = current_ref!(f.editor).1.id();
    std::fs::write(&path, "disk\n")?;
    f.editor.reload_document(id)?;
    let task = stash(&f)?.prepare().await?.admit(&mut f.editor)?;
    f.replace("newer unsaved edits\n");
    task.execute().await.finish(&mut f.editor);
    assert_eq!(
        current_ref!(f.editor).1.text().to_string(),
        "newer unsaved edits\n"
    );
    assert!(current_ref!(f.editor).1.is_modified());
    assert_eq!(std::fs::read_to_string(&path)?, "head\n");
    f.editor.save(id, None::<PathBuf>, true)?;
    f.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(path)?, "newer unsaved edits\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn staging_uses_disk_preserves_unsaved_edits_and_waits_for_existing_saves(
) -> anyhow::Result<()> {
    let mut f = fixture()?;
    let path = path(&f);
    let id = current_ref!(f.editor).1.id();
    std::fs::write(&path, "disk\n")?;
    f.editor.reload_document(id)?;
    f.replace("buffer\n");
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path: path.clone() }),
    };
    let action = GitAction::File(status, FileOperation::Stage);
    let task = GitRequest::capture(&f.editor, f.dir.path().into(), action.clone())?
        .prepare()
        .await?
        .admit(&mut f.editor)?;
    task.execute().await.finish(&mut f.editor);
    assert_eq!(current_ref!(f.editor).1.text().to_string(), "buffer\n");
    assert_eq!(std::fs::read_to_string(&path)?, "disk\n");
    let prepared = GitRequest::capture(&f.editor, f.dir.path().into(), action)?
        .prepare()
        .await?;
    f.editor.save(id, None::<PathBuf>, false)?;
    assert!(prepared.admit(&mut f.editor).is_err());
    f.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(path)?, "buffer\n");
    Ok(())
}
