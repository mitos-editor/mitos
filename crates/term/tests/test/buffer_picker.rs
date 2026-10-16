use std::time::{Duration, Instant};

use editor_core::Transaction;
use term::{
    application::Application,
    config::{Config, ConfigLoadError},
};
use ui_core::input::parse_macro;
use view::{current, current_ref, editor::Action, DocumentId};

use super::helpers::{run_event_loop_until_idle, test_config, AppBuilder};

struct Fixture {
    app: Application,
    _dir: tempfile::TempDir,
    ids: Vec<DocumentId>,
}

impl Fixture {
    fn new(names: &[&str]) -> anyhow::Result<Self> {
        let mut config = test_config();
        config.component_keys = Config::load(
            Ok(&r#"
                [keys.picker]
                C-j = "move_down"
                C-space = "accept"
                [keys.buffer-picker]
                C-x = "close_buffer"
            "#
            .to_owned()),
            Err(ConfigLoadError::default()),
        )
        .unwrap()
        .component_keys;
        let dir = tempfile::tempdir()?;
        let mut app = AppBuilder::new().with_config(config).build()?;
        let mut ids = Vec::new();
        for name in names {
            let path = dir.path().join(name);
            std::fs::write(&path, "content\n")?;
            ids.push(app.editor.open(&path, Action::Replace)?);
        }
        // Give the buffer rows deterministic MRU order, with the current one first.
        let now = Instant::now();
        for (index, &id) in ids.iter().enumerate() {
            app.editor.document_mut(id).unwrap().focused_at =
                now - Duration::from_secs(index as u64 + 1);
        }
        if let Some(&id) = ids.last() {
            app.editor.document_mut(id).unwrap().focused_at = now;
        }
        Ok(Self {
            app,
            _dir: dir,
            ids,
        })
    }

    async fn keys(&mut self, keys: &str) -> anyhow::Result<()> {
        for key in parse_macro(keys)? {
            #[cfg(not(windows))]
            let event = termina::event::Event::Key(key.into());
            #[cfg(windows)]
            let event = crossterm::event::Event::Key(key.into());
            self.app.handle_terminal_events(Ok(event)).await;
        }
        // Mirror the event loop resetting its timer after each input event.
        self.app.editor.reset_idle_timer();
        // Also render asynchronous matching and lifecycle callbacks before asserting.
        tokio::time::timeout(
            Duration::from_secs(10),
            run_event_loop_until_idle(&mut self.app),
        )
        .await?;
        Ok(())
    }

    fn current(&self) -> DocumentId {
        current_ref!(self.app.editor).1.id()
    }

    fn screen(&self) -> String {
        self.app
            .terminal_backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    async fn finish(&mut self) {
        assert!(self.app.close().await.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn close_selected_buffer_preserves_query_and_nearest_row() -> anyhow::Result<()> {
    for (movement, closed_index, accepted_index) in [("<C-j>", 1, 2), ("<end>", 2, 1)] {
        let mut f = Fixture::new(&[
            "pick-aaa.txt",
            "pick-bbb.txt",
            "pick-ccc.txt",
            "unrelated.txt",
        ])?;
        let current = f.current();
        f.keys("<space>b").await?;
        f.keys("pick").await?;
        f.keys(movement).await?;
        f.keys("<C-x>").await?;
        assert!(
            f.app.editor.document(f.ids[closed_index]).is_none(),
            "movement {movement}, screen: {}",
            f.screen()
        );
        assert_eq!(f.current(), current); // Closing an inactive row does not switch focus.
        assert!(!f.screen().contains(if closed_index == 1 {
            "pick-bbb.txt"
        } else {
            "pick-ccc.txt"
        }));
        f.keys("<C-space>").await?;
        // If the query had been cleared this would select the unrelated current
        // buffer; if the cursor had been reset this would select the first row.
        assert_eq!(f.current(), f.ids[accepted_index]);
        f.finish().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn modified_selected_buffer_reports_error_and_keeps_picker_open() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["pick-modified.txt", "unrelated.txt"])?;
    f.app.editor.switch(f.ids[0], Action::Replace);
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("changed".into()))].into_iter());
    view::editing::apply(doc, &transaction, view.id);
    f.app.editor.switch(f.ids[1], Action::Replace);
    f.keys("<space>b").await?;
    f.keys("pick").await?;
    f.keys("<C-x>").await?;
    assert!(f.app.editor.document(f.ids[0]).unwrap().is_modified());
    assert_eq!(f.current(), f.ids[1]);
    assert!(f
        .app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("unsaved changes"));
    f.keys("<C-space>").await?;
    assert_eq!(f.current(), f.ids[0]); // The same row is still selectable.
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn close_selected_buffer_waits_for_pending_save() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["pick-saved.txt", "unrelated.txt"])?;
    f.app.editor.switch(f.ids[0], Action::Replace);
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("saved ".into()))].into_iter());
    view::editing::apply(doc, &transaction, view.id);
    let path = doc.path().unwrap().to_owned();
    f.app.editor.switch(f.ids[1], Action::Replace);
    f.keys("<space>b").await?;
    f.keys("pick").await?;
    f.app
        .editor
        .save(f.ids[0], None::<std::path::PathBuf>, false)?;
    assert!(f.app.editor.has_pending_writes());
    f.keys("<C-x>").await?;
    assert!(!f.app.editor.has_pending_writes());
    assert!(f.app.editor.document(f.ids[0]).is_none());
    assert_eq!(std::fs::read_to_string(path)?, "saved content\n");
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_final_buffer_keeps_editor_and_picker_usable() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[])?;
    let original = f.current();
    assert_eq!(f.app.editor.documents().count(), 1);
    f.keys("<space>b").await?;
    f.keys("<C-x>").await?;
    assert!(f.app.editor.document(original).is_none());
    assert_eq!(f.app.editor.documents().count(), 1);
    assert_ne!(f.current(), original);
    let replacement = f.current();
    f.keys("<C-x>").await?; // Refreshed picker contains the new scratch buffer.
    assert!(f.app.editor.document(replacement).is_none());
    assert_eq!(f.app.editor.documents().count(), 1);
    f.keys("<esc>").await?;
    f.keys("ix<esc>").await?;
    assert!(current_ref!(f.app.editor)
        .1
        .text()
        .to_string()
        .starts_with('x'));
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_buffer_row_refreshes_and_empty_selection_does_nothing() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["pick-stale.txt", "unrelated.txt"])?;
    f.keys("<space>b").await?;
    f.keys("pick").await?;
    assert!(f.app.editor.close_document(f.ids[0], false).is_ok());
    let current = f.current();
    f.keys("<C-x>").await?;
    assert!(!f.screen().contains("pick-stale.txt"));
    f.keys("<C-x>").await?;
    assert_eq!(f.current(), current);
    assert!(f.app.editor.document(f.ids[1]).is_some());
    f.keys("<esc>").await?;
    f.keys("ix<esc>").await?;
    assert!(current_ref!(f.app.editor)
        .1
        .text()
        .to_string()
        .starts_with('x'));
    f.finish().await;
    Ok(())
}
