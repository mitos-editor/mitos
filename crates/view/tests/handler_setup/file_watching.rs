use anyhow::Context as _;
use std::time::Duration;
use view::current_ref;

use super::Fixture;

#[tokio::test(flavor = "multi_thread")]
async fn native_file_watching_reloads_without_terminal_setup() -> anyhow::Result<()> {
    let mut f = Fixture::new("before\n")?;
    let mut events = f.editor.file_watcher.subscribe();
    f.configure(|config| {
        config.files.reload.enable = true;
        config.files.watch.enable = true;
    });
    f.editor.file_watcher.add_root(f.dir.path());
    let path = f.dir.path().canonicalize()?.join("document.words");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.editor.file_watcher.is_watching(&path) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("native root did not become ready")?;
    let replacement = f.dir.path().join("replacement");
    std::fs::write(&replacement, "after\n")?;
    std::fs::rename(replacement, &path)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(f.editor).1.text() != "after\n" {
            let callback = f
                .callbacks
                .recv()
                .await
                .context("editor callback queue closed")?;
            callback(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await
    .context("native change did not reach its headless editor")??;
    assert!(!current_ref!(f.editor).1.is_modified());
    assert!(std::iter::from_fn(|| events.try_recv().ok())
        .any(|events| { events.iter().any(|event| event.path.as_std_path() == path) }));
    Ok(())
}

#[tokio::test]
async fn subscriptions_follow_only_their_editor_and_survive_watch_reconfiguration(
) -> anyhow::Result<()> {
    use view::file_watcher::{CanonicalPathBuf, Event, EventType, Events};

    let mut first = Fixture::new("first\n")?;
    let second = Fixture::new("second\n")?;
    let mut first_events = first.editor.file_watcher.subscribe();
    let mut second_events = second.editor.file_watcher.subscribe();
    first.configure(|config| config.files.watch.hidden = !config.files.watch.hidden);
    let events = Events::from(vec![Event {
        path: CanonicalPathBuf::assert_canonicalized(
            &first.dir.path().canonicalize()?.join("document.words"),
        ),
        ty: EventType::Modified,
    }]);
    first.editor.handle_file_events(&events);
    assert_eq!(first_events.try_recv()?, events);
    assert!(matches!(
        second_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    Ok(())
}

#[cfg(feature = "git")]
fn git(root: &std::path::Path, args: &[&str]) -> anyhow::Result<()> {
    let result = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Mitos test",
            "-c",
            "user.email=test@mitos.org",
        ])
        .args(args)
        .env_remove("GIT_DIR")
        .output()?;
    anyhow::ensure!(
        result.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}

#[cfg(feature = "git")]
async fn assert_native_index_changes(linked: bool) -> anyhow::Result<()> {
    use view::editor::Action;
    let mut f = Fixture::new("tracked\n")?;
    let main = f.dir.path().canonicalize()?;
    git(&main, &["init", "-q"])?;
    let worktree = if linked {
        git(&main, &["add", "document.words"])?;
        git(&main, &["commit", "-qm", "initial"])?;
        git(&main, &["worktree", "add", "-qb", "linked", "linked"])?;
        main.join("linked")
    } else {
        main.clone()
    };
    f.editor
        .open(&worktree.join("document.words"), Action::Load)?;
    f.editor.refresh_vcs_watches();
    let index = f
        .editor
        .diff_providers
        .get_watched_paths(&worktree, false)
        .into_iter()
        .find(|path| path.file_name().is_some_and(|name| name == "index"))
        .unwrap();
    f.configure(|config| {
        config.files.watch.enable = true;
        config.files.watch.watch_vcs = true;
        config.files.reload.poll.enable = false;
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.editor.file_watcher.is_watching(&index) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("native index root did not become ready")?;
    let mut events = f.editor.file_watcher.subscribe();
    if linked {
        assert!(!index.starts_with(&worktree));
        std::fs::write(worktree.join("document.words"), "modified\n")?;
    } else {
        assert!(!index.exists(), "exercise creation of the first index");
    }
    for args in [
        &["add", "document.words"][..],
        &["rm", "--cached", "document.words"][..],
    ] {
        git(&worktree, args)?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let callback = f
                    .callbacks
                    .recv()
                    .await
                    .context("editor callback queue closed")?;
                callback(&mut f.editor);
                if std::iter::from_fn(|| events.try_recv().ok())
                    .any(|batch| batch.iter().any(|event| event.path.as_std_path() == index))
                {
                    return anyhow::Ok(());
                }
            }
        })
        .await
        .context("index change was not delivered without polling")??;
    }
    let attributes = f
        .editor
        .diff_providers
        .get_watched_paths(&worktree, false)
        .into_iter()
        .find(|path| path.ends_with("info/attributes"))
        .unwrap();
    std::fs::write(&attributes, "* text eol=crlf\n")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let callback = f
                .callbacks
                .recv()
                .await
                .context("editor callback queue closed")?;
            callback(&mut f.editor);
            if std::iter::from_fn(|| events.try_recv().ok()).any(|batch| {
                batch
                    .iter()
                    .any(|event| event.path.as_std_path() == attributes)
            }) {
                return anyhow::Ok(());
            }
        }
    })
    .await
    .context("repository attributes change was not delivered without polling")??;
    Ok(())
}

#[cfg(feature = "git")]
#[tokio::test(flavor = "multi_thread")]
async fn native_index_creation_and_staging_work_without_polling() -> anyhow::Result<()> {
    assert_native_index_changes(false).await
}

#[cfg(feature = "git")]
#[tokio::test(flavor = "multi_thread")]
async fn linked_worktree_index_changes_work_without_polling() -> anyhow::Result<()> {
    assert_native_index_changes(true).await
}
