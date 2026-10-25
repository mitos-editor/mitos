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
    config: Config,
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
            config: Config::default(),
        })
    }

    fn event(&mut self, event: Event) {
        let config = &self.config;
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        self.compositor.handle_event(
            &event,
            &mut UiContext {
                config: crate::config::Context {
                    current: config,
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
        self.frame_at(120, 24)
    }

    fn frame_at(&mut self, width: u16, height: u16) -> String {
        let config = &self.config;
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let area = Rect::new(0, 0, width, height);
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
                    current: config,
                    updates: &updates,
                },
                editor: &mut self.app.editor,
                jobs: &mut self.jobs,
                scroll: None,
                image_picker: None,
                is_cursor_owner: true,
            },
        );
        let _ = picker.cursor(area, &self.app.editor);
        surface
            .content
            .chunks(usize::from(width).max(1))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn settle(&mut self) -> anyhow::Result<()> {
        self.settle_with_timeout(Duration::from_secs(5)).await
    }

    async fn settle_with_timeout(&mut self, timeout: Duration) -> anyhow::Result<()> {
        tokio::time::timeout(timeout, async {
            loop {
                let config = &self.config;
                let updates = tokio::sync::mpsc::unbounded_channel().0;
                while let Ok(callback) = self.jobs.callbacks.try_recv() {
                    self.jobs.handle_callback(
                        crate::config::Context {
                            current: config,
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
async fn replacement_hints_follow_current_bindings_and_picker_state() -> anyhow::Result<()> {
    use crate::keymap::component::PickerAction;
    use ui_core::keymap::{Binding, Keymap};

    let dir = tempfile::tempdir()?;
    let mut h = Harness::new(dir.path())?;
    let frame = h.frame_at(200, 24);
    assert!(frame.contains("<A-r> replace"));
    assert!(frame.contains("<A-c> toggle case"));
    assert!(!frame.contains("replace all"));
    h.key("A-r");
    let frame = h.frame_at(200, 24);
    assert!(frame.contains("<A-r> switch input"));
    assert!(frame.contains("<ret> replace one"));
    assert!(frame.contains("<C-ret> replace all"));

    let keys = &mut h.config.component_keys.picker.0;
    keys.insert("A-r".parse()?, Binding::Unbound);
    keys.insert(
        "F4".parse()?,
        Binding::Actions(vec![PickerAction::ToggleReplacement]),
    );
    keys.insert("A-c".parse()?, Binding::Unbound);
    keys.insert("C-ret".parse()?, Binding::Unbound);
    keys.insert("Meta-ret".parse()?, Binding::Unbound);
    keys.insert(
        "F5".parse()?,
        Binding::Keys(Keymap(std::collections::HashMap::from([(
            "a".parse()?,
            Binding::Actions(vec![PickerAction::ReplaceAll]),
        )]))),
    );
    let frame = h.frame_at(200, 24);
    assert!(frame.contains("<F4> switch input"));
    assert!(frame.contains("<F5>a replace all"));
    assert!(!frame.contains("<A-r>"));
    assert!(!frame.contains("toggle case"));
    Ok(())
}

#[tokio::test]
async fn replacement_footer_keeps_inputs_and_results_visible_on_small_terminals(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("entry.txt"), "foo\n")?;
    let mut h = Harness::new(dir.path())?;
    h.event(Event::Paste("foo".into()));
    h.settle().await?;
    h.key("A-r");
    h.event(Event::Paste("bar".into()));
    let frame = h.frame_at(40, 12);
    for text in [
        "search: foo",
        "replace: bar",
        "entry.txt",
        "<ret> replace one",
    ] {
        assert!(frame.contains(text), "missing {text:?} in {frame}");
    }
    assert!(!frame.contains("<C-ret> replace all"));
    assert!(!frame.contains("<A-r> switch input"));
    let rows: Vec<_> = frame.lines().collect();
    assert!(rows.last().unwrap().contains("<ret> replace one"));
    assert!(rows[..rows.len() - 1]
        .iter()
        .all(|row| !row.contains("<ret>") && !row.contains("<C-ret>")));
    // Layout clipping must also remain safe when a terminal is very small.
    h.key("C-a");
    for width in [0, 1, 2, 4, 5, 20, 40, 73] {
        for height in [0, 1, 2, 4, 6, 8] {
            h.frame_at(width, height);
        }
    }
    Ok(())
}

#[tokio::test]
async fn single_replace_refreshes_only_its_buffer_and_preserves_all_other_results(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for index in 0..40 {
        std::fs::write(
            dir.path().join(format!("{index:02}.txt")),
            "α foo\nfoo\nfoo\n",
        )?;
    }
    let mut h = Harness::new(dir.path())?;
    h.event(Event::Paste("foo".into()));
    h.settle().await?;
    assert_eq!(h.count(), 120);
    let selected = h.picker().selection().unwrap().clone();
    assert_eq!(selected.line_start, 0);
    let unchanged: Vec<_> = test::items(h.picker())
        .filter(|item| item.path != selected.path)
        .cloned()
        .collect();
    // With watching/polling disabled, a silent disk edit must not be read during
    // replacement of another file. An explicit filesystem event refreshes it below.
    let other = dir.path().join(unchanged[0].path.file_name().unwrap());
    std::fs::write(&other, "no matches\n")?;
    h.key("A-r");
    h.event(Event::Paste("bar\nnew".into()));
    h.key("ret");
    h.settle().await?;
    assert_eq!(h.count(), 119);
    let remaining: Vec<_> = test::items(h.picker()).cloned().collect();
    assert_eq!(
        remaining
            .iter()
            .filter(|item| item.path != selected.path)
            .cloned()
            .collect::<Vec<_>>(),
        unchanged
    );
    let changed: Vec<_> = remaining
        .iter()
        .filter(|item| item.path == selected.path)
        .collect();
    assert_eq!(changed.len(), 2);
    // Inserting a newline shifts all following matches in the edited buffer.
    assert_eq!(
        changed
            .iter()
            .map(|item| item.line_start)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    h.app
        .editor
        .handle_file_events(&view::file_watcher::Events::from(vec![
            view::file_watcher::Event {
                path: view::file_watcher::CanonicalPathBuf::assert_canonicalized(
                    &stdx::path::canonicalize_existing(&other),
                ),
                ty: view::file_watcher::EventType::Modified,
            },
        ]));
    h.settle().await?;
    assert_eq!(h.count(), 116);
    assert!(test::items(h.picker()).all(|item| item.path.file_name() != other.file_name()));
    Ok(())
}

#[tokio::test]
async fn replace_all_with_no_remaining_matches_still_publishes_the_buffer_refresh(
) -> anyhow::Result<()> {
    let dir = tempfile::Builder::new()
        .prefix("included-parent-")
        .tempdir()?;
    let included = dir.path().join("included.txt");
    let excluded = dir.path().join("excluded.txt");
    std::fs::write(&included, "foo\n")?;
    std::fs::write(&excluded, "foo\n")?;
    let mut h = Harness::new(dir.path())?;
    h.event(Event::Paste("foo %p 'included.txt".into()));
    h.settle().await?;
    assert_eq!(h.count(), 1);
    assert_eq!(test::all_items(h.picker()).count(), 2);
    // Neither file is open or watched. Replace-all should discover that the
    // included match disappeared, without reading the excluded file again.
    std::fs::write(&included, "gone\n")?;
    std::fs::write(&excluded, "gone\n")?;
    h.key("A-r");
    h.event(Event::Paste("bar".into()));
    h.key("C-ret");
    h.settle().await?;
    assert_eq!(h.count(), 0);
    assert_eq!(test::all_items(h.picker()).count(), 1);
    assert_eq!(
        test::all_items(h.picker()).next().unwrap().path.file_name(),
        excluded.file_name()
    );
    assert!(h
        .app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("No matches replaced"));
    Ok(())
}

#[tokio::test]
async fn bulk_replacement_keeps_navigation_responsive_and_does_not_open_files() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    for index in 0..128 {
        std::fs::write(
            dir.path().join(format!("{index:03}.txt")),
            "foo\n".repeat(8),
        )?;
    }
    let mut h = Harness::new(dir.path())?;
    h.event(Event::Paste("foo".into()));
    h.settle().await?;
    assert_eq!(h.count(), 1024);
    let documents = h.app.editor.documents().count();
    h.key("A-r");
    h.event(Event::Paste("bar".into()));
    h.key("C-ret");
    assert!(!test::is_settled(h.picker()));
    let selected = h.picker().selection().unwrap().clone();
    h.key("down");
    assert_ne!(h.picker().selection(), Some(&selected));
    h.event(Event::Paste("ignored while replacing".into()));
    assert!(!h.frame().contains("ignored while replacing"));
    h.settle_with_timeout(Duration::from_secs(30)).await?;
    assert_eq!(h.count(), 0);
    assert_eq!(h.app.editor.documents().count(), documents);
    for index in 0..128 {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(format!("{index:03}.txt")))?,
            "bar\n".repeat(8)
        );
    }
    Ok(())
}

#[tokio::test]
async fn closing_before_the_apply_step_cancels_without_writing_files() -> anyhow::Result<()> {
    use futures_util::StreamExt;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "foo\n")?;
    let mut h = Harness::new(dir.path())?;
    h.event(Event::Paste("foo".into()));
    h.settle().await?;
    let documents = h.app.editor.documents().count();
    h.key("A-r");
    h.event(Event::Paste("bar".into()));
    h.key("ret");
    h.key("esc");
    while let Some(result) = h.jobs.wait_futures.next().await {
        result?;
    }
    assert_eq!(std::fs::read_to_string(&path)?, "foo\n");
    assert_eq!(h.app.editor.documents().count(), documents);
    Ok(())
}

#[tokio::test]
async fn replacement_refreshes_shifted_matches_and_respects_path_filters() -> anyhow::Result<()> {
    // Parent directories must not satisfy the filename filter.
    let dir = tempfile::Builder::new()
        .prefix("included-parent-")
        .tempdir()?;
    let path = dir.path().join("included.txt");
    let excluded = dir.path().join("excluded.txt");
    std::fs::write(&path, "α foo foo\nfoo\n")?;
    std::fs::write(&excluded, "foo\n")?;
    // Alternate spellings of the same root must still use open-buffer snapshots.
    std::fs::create_dir(dir.path().join("root-alias"))?;
    let mut h = Harness::new(&dir.path().join("root-alias/.."))?;
    h.app.editor.open(&path, view::editor::Action::Load)?;
    h.event(Event::Paste("foo %p 'included.txt".into()));
    h.settle().await?;
    assert_eq!(h.count(), 2);
    h.key("A-r");
    h.event(Event::Paste("longer".into()));
    h.key("ret");
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
    h.key("A-r");
    h.key("C-a");
    h.key("C-k");
    h.event(Event::Paste("foo".into()));
    h.settle().await?;
    assert_eq!(h.count(), 1);
    assert_eq!(
        h.picker().selection().unwrap().path.file_name(),
        excluded.file_name()
    );
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
