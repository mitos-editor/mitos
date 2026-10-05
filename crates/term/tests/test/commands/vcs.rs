use std::{path::Path, process::Command, sync::Arc, time::Duration};

use view::{config::InlineBlameShow, current_ref};

use term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;
use ui_core::input::parse_macro;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::super::helpers::{run_event_loop_until_idle, AppBuilder};

async fn press_space_b(app: &mut Application) -> anyhow::Result<()> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro("<space>B")? {
        sender.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut input = UnboundedReceiverStream::new(receiver);
    assert!(app.event_loop_until_idle(&mut input).await);
    Ok(())
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn space_b_toggles_inline_blame_and_reuses_the_cached_snapshot() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "committed\n")?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "file.txt"]);
    git(dir.path(), &["commit", "-m", "initial"]);
    let mut app = AppBuilder::new().with_file(path, None).build()?;
    assert!(!app.editor.config().inline_blame.auto_fetch);
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::Never
    );
    assert!(current_ref!(app.editor).1.file_blame().is_none());

    press_space_b(&mut app).await?;
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::CursorLine
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(app.editor).1.file_blame().is_none() {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    let original = current_ref!(app.editor)
        .1
        .file_blame()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(
        current_ref!(app.editor).1.line_blame(0, "{title}").unwrap(),
        "initial"
    );

    press_space_b(&mut app).await?;
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::Never
    );
    press_space_b(&mut app).await?;
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::CursorLine
    );
    let current = current_ref!(app.editor)
        .1
        .file_blame()
        .unwrap()
        .as_ref()
        .unwrap();
    assert!(Arc::ptr_eq(&original, current));
    assert!(!app.editor.config().inline_blame.auto_fetch);
    assert!(app.close().await.is_empty());
    Ok(())
}
