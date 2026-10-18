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
