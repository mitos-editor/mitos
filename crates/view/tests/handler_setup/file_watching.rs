use anyhow::Context as _;
use std::time::Duration;
use view::current_ref;

use super::Fixture;

#[tokio::test(flavor = "multi_thread")]
async fn native_file_watching_reloads_without_terminal_setup() -> anyhow::Result<()> {
    let mut f = Fixture::new("before\n")?;
    f.configure(|config| {
        config.auto_reload.enable = true;
        config.file_watcher.enable = true;
    });
    f.editor.file_watcher.add_root(f.dir.path());
    let path = f.dir.path().join("document.words");
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
    Ok(())
}
