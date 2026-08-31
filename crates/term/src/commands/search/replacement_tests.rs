use super::*;
use crate::{
    application::Application,
    compositor::{Component, Compositor, Context as UiContext, Event},
    config::Config,
    job::Jobs,
    ui::{overlay::Overlay, picker::test},
};
use std::time::Duration;
use view::graphics::Rect;

type SearchPicker = Picker<::search::Match, GlobalSearchConfig>;

struct Harness {
    app: Application,
    compositor: Compositor,
    jobs: Jobs,
}

impl Harness {
    fn new(root: &std::path::Path) -> anyhow::Result<Self> {
        let mut app = test::application(false)?;
        let jobs = Jobs::new();
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let picker = global_search_picker(&mut app.editor, &jobs, root.into(), '/');
        compositor.push(Box::new(overlaid(picker)));
        Ok(Self {
            app,
            compositor,
            jobs,
        })
    }

    fn event(&mut self, event: Event) {
        let config = Config::default();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        self.compositor.handle_event(
            &event,
            &mut UiContext {
                config: crate::config::Context {
                    current: &config,
                    updates: &updates,
                },
                editor: &mut self.app.editor,
                jobs: &mut self.jobs,
                scroll: None,
                image_picker: None,
                is_cursor_owner: true,
            },
        );
    }

    fn key(&mut self, key: &str) {
        self.event(Event::Key(key.parse().unwrap()));
    }

    fn picker(&mut self) -> &mut SearchPicker {
        &mut self
            .compositor
            .find::<Overlay<SearchPicker>>()
            .unwrap()
            .content
    }

    fn frame(&mut self) -> String {
        let config = Config::default();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let area = Rect::new(0, 0, 120, 24);
        let mut surface = tui::buffer::Buffer::empty(area);
        let picker = &mut self
            .compositor
            .find::<Overlay<SearchPicker>>()
            .unwrap()
            .content;
        picker.render(
            area,
            &mut surface,
            &mut UiContext {
                config: crate::config::Context {
                    current: &config,
                    updates: &updates,
                },
                editor: &mut self.app.editor,
                jobs: &mut self.jobs,
                scroll: None,
                image_picker: None,
                is_cursor_owner: true,
            },
        );
        surface.content.iter().map(|cell| cell.symbol()).collect()
    }

    async fn settle(&mut self) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let config = Config::default();
                let updates = tokio::sync::mpsc::unbounded_channel().0;
                while let Ok(callback) = self.jobs.callbacks.try_recv() {
                    self.jobs.handle_callback(
                        crate::config::Context {
                            current: &config,
                            updates: &updates,
                        },
                        &mut self.app.editor,
                        &mut self.compositor,
                        Ok(Some(callback)),
                    );
                }
                let picker = &mut self
                    .compositor
                    .find::<Overlay<SearchPicker>>()
                    .unwrap()
                    .content;
                test::draw(picker, &mut self.app);
                if test::is_settled(picker) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        Ok(())
    }

    fn count(&mut self) -> usize {
        test::items(self.picker()).count()
    }
}

#[tokio::test]
async fn replacement_refreshes_shifted_matches_and_respects_path_filters() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("included.txt");
    let excluded = dir.path().join("excluded.txt");
    std::fs::write(&path, "α foo foo\nfoo\n")?;
    std::fs::write(&excluded, "foo\n")?;
    // Alternate spellings of the same root must still use open-buffer snapshots.
    std::fs::create_dir(dir.path().join("root-alias"))?;
    let mut h = Harness::new(&dir.path().join("root-alias/.."))?;
    h.event(Event::Paste("foo %p included".into()));
    h.settle().await?;
    assert_eq!(h.count(), 2);
    h.key("A-r");
    h.event(Event::Paste("longer".into()));
    h.key("ret");
    // A second action before refreshed coordinates are published cannot edit stale rows.
    h.key("ret");
    assert!(h
        .app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("Wait for global search"));
    h.settle().await?;
    let id = h.app.editor.open(&path, view::editor::Action::Load)?;
    assert_eq!(
        h.app.editor.document(id).unwrap().text(),
        "α longer foo\nfoo\n"
    );
    assert_eq!(
        test::items(h.picker())
            .find(|item| item.line_start == 0)
            .unwrap()
            .match_start_col,
        9
    );
    assert!(h.frame().contains("α longer foo"));
    h.key("esc");
    let cached = h.compositor.last_picker.take().unwrap();
    h.compositor.push(cached);
    h.settle().await?;
    assert!(h.frame().contains("α longer foo"));
    h.key("C-ret");
    h.settle().await?;
    assert_eq!(h.count(), 0);
    assert_eq!(
        h.app.editor.document(id).unwrap().text(),
        "α longer longer\nlonger\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, "α foo foo\nfoo\n");
    assert_eq!(std::fs::read_to_string(&excluded)?, "foo\n");
    h.app.editor.open(&path, view::editor::Action::Replace)?;
    assert!(h.app.editor.change_history(buffer::HistoryAction::Undo));
    assert_eq!(
        h.app.editor.document(id).unwrap().text(),
        "α longer foo\nfoo\n"
    );
    assert!(h.app.editor.change_history(buffer::HistoryAction::Undo));
    assert_eq!(
        h.app.editor.document(id).unwrap().text(),
        "α foo foo\nfoo\n"
    );
    Ok(())
}

#[tokio::test]
async fn case_toggle_requeries_and_replacement_prompt_owns_register_input() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "foo\nFOO\n")?;
    let mut h = Harness::new(dir.path())?;
    h.event(Event::Paste("foo".into()));
    h.settle().await?;
    assert_eq!(h.count(), 2);
    h.key("A-c");
    h.key("A-r");
    h.event(Event::Paste("bar".into()));
    h.key("C-ret");
    // Case changes cannot reuse the previous smart-case result generation.
    assert!(h
        .app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("Wait for global search"));
    h.settle().await?;
    assert_eq!(h.count(), 1);
    h.app.editor.registers.push('r', "!".into())?;
    h.key("C-r");
    h.key("r");
    h.key("Meta-ret");
    h.settle().await?;
    let id = h.app.editor.open(&path, view::editor::Action::Load)?;
    assert_eq!(h.app.editor.document(id).unwrap().text(), "bar!\nFOO\n");
    // Once search has focus again, edits affect the query, not replacement text.
    h.key("A-r");
    h.key("C-a");
    h.key("C-k");
    h.event(Event::Paste("FOO".into()));
    h.settle().await?;
    assert_eq!(h.count(), 1);
    Ok(())
}

#[tokio::test]
async fn closing_during_case_refresh_keeps_reopened_replacement_usable() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "foo\nFOO\n")?;
    let mut h = Harness::new(dir.path())?;
    h.event(Event::Paste("foo".into()));
    h.settle().await?;
    h.key("A-c");
    h.key("esc");
    // Let the queued refresh run while the picker is hidden, as it would
    // while editing a document before reopening the cached picker.
    let callback = tokio::time::timeout(Duration::from_secs(5), h.jobs.callbacks.recv())
        .await?
        .unwrap();
    h.jobs.handle_callback(
        crate::config::Context {
            current: &Config::default(),
            updates: &tokio::sync::mpsc::unbounded_channel().0,
        },
        &mut h.app.editor,
        &mut h.compositor,
        Ok(Some(callback)),
    );
    let cached = h.compositor.last_picker.take().unwrap();
    h.compositor.push(cached);
    h.settle().await?;
    assert_eq!(h.count(), 1);
    h.key("A-r");
    h.event(Event::Paste("bar".into()));
    h.key("ret");
    h.settle().await?;
    let id = h.app.editor.open(&path, view::editor::Action::Load)?;
    assert_eq!(h.app.editor.document(id).unwrap().text(), "bar\nFOO\n");
    Ok(())
}
