use super::helpers::{run_event_loop_until_idle, AppBuilder};
use std::{path::Path, process::Command, time::Duration};
use term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;
use ui_core::input::parse_macro;
use view::{current_ref, editor::Action};

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo() -> anyhow::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init"]);
    std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
    std::fs::write(dir.path().join("a.txt"), "head\n")?;
    std::fs::write(dir.path().join("b.txt"), "other\n")?;
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    Ok(dir)
}

async fn keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut input = UnboundedReceiverStream::new(rx);
    assert!(app.event_loop_until_idle(&mut input).await);
    Ok(())
}

async fn ready(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(app).await;
            if current_ref!(app.editor)
                .1
                .project_review()
                .is_some_and(|state| !state.loading)
            {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn project_review_separates_layers_refreshes_and_opens_file_reviews() -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "staged\n")?;
    git(dir.path(), &["add", "a.txt"]);
    std::fs::write(&path, "disk\n")?;
    std::fs::write(dir.path().join("new.txt"), "untracked\n")?;
    std::fs::write(dir.path().join("binary.bin"), b"\0binary")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    keys(&mut app, "Ibuffer-<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "buffer-disk\n"
    );
    keys(&mut app, ":vcs-project-diff<ret>").await?;
    ready(&mut app).await?;
    let id = current_ref!(app.editor).1.id();
    let text = current_ref!(app.editor).1.text().to_string();
    assert!(text.contains("-staged"), "{text}");
    assert!(text.contains("+buffer-disk"), "{text}");
    assert!(text.contains("new.txt"));
    assert!(text.contains("binary.bin") && text.contains("Cannot review:"));
    assert!(current_ref!(app.editor).1.is_content_readonly());
    keys(&mut app, "iSHOULD-NOT-EDIT<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), text);
    assert_eq!(std::fs::read_to_string(&path)?, "disk\n");
    keys(&mut app, ":vcs-project-next file<ret><ret>").await?;
    assert!(current_ref!(app.editor).1.review().is_some());
    keys(&mut app, "<C-o>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), id);
    std::fs::write(dir.path().join("new.txt"), "refreshed\n")?;
    keys(&mut app, ":vcs-project-refresh<ret>").await?;
    ready(&mut app).await?;
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("+refreshed"));
    keys(&mut app, ":vcs-project-diff staged<ret>").await?;
    ready(&mut app).await?;
    let text = current_ref!(app.editor).1.text().to_string();
    assert!(text.contains("-head") && text.contains("+staged"), "{text}");
    assert!(!text.contains("new.txt") && !text.contains("buffer-disk"));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn project_review_includes_clean_files_with_unsaved_changes_and_cancels_closed_work(
) -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    let source = current_ref!(app.editor).1.id();
    keys(&mut app, "Iunsaved-<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "unsaved-head\n"
    );
    keys(&mut app, ":vcs-project-diff<ret>").await?;
    ready(&mut app).await?;
    let review = current_ref!(app.editor).1.id();
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("+unsaved-head"));
    assert_eq!(std::fs::read_to_string(&path)?, "head\n");
    keys(&mut app, ":vcs-project-refresh<ret>").await?;
    app.editor.switch(source, Action::Replace);
    app.editor
        .close_document(review, false)
        .map_err(|_| anyhow::anyhow!("close failed"))?;
    run_event_loop_until_idle(&mut app).await;
    assert!(app.editor.document(review).is_none());
    git(dir.path(), &["diff", "--exit-code"]);
    assert!(app.close().await.is_empty());
    Ok(())
}
