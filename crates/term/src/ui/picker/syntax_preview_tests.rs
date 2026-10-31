use std::time::Duration;

use language::syntax;

use super::*;
use crate::{
    application::Application,
    args::Args,
    config::Config,
    job::{Callback, Jobs},
    ui::overlay::{overlaid, Overlay},
};
use crate::{compositor::Compositor, ui::picker::Column};
use view::editor::Action;

struct Fixture {
    app: Application,
    jobs: Jobs,
    compositor: Compositor,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> anyhow::Result<Self> {
        Self::with_config(|_| {})
    }

    fn with_config(configure: impl FnOnce(&mut Config)) -> anyhow::Result<Self> {
        let mut config = Config::default();
        config.editor.lsp.enable = false;
        config.editor.files.watch.enable = false;
        config.editor.files.reload.enable = false;
        config.editor.completion.word.enable = false;
        configure(&mut config);
        let loader = syntax::Loader::new(
            toml::from_str(
                r#"
                [[language]]
                name = "json"
                scope = "source.json"
                file-types = ["json"]
                "#,
            )?,
            loader::syntax::Resources::default(),
        )?;
        let app = Application::new(
            Args::default(),
            config,
            loader,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;
        let picker = Picker::new(
            app.editor.redraw().clone(),
            [Column::new("name", |_: &(), _| "".into())],
            0,
            [()],
            (),
            |_, _, _| {},
        );
        let mut compositor = Compositor::new(Rect::new(0, 0, 100, 30));
        compositor.push(Box::new(overlaid(picker)));
        Ok(Self {
            app,
            jobs: Jobs::new(),
            compositor,
            dir: tempfile::tempdir()?,
        })
    }

    fn picker(&mut self) -> &mut Picker<(), ()> {
        &mut self
            .compositor
            .find::<Overlay<Picker<(), ()>>>()
            .unwrap()
            .content
    }

    fn preview(&mut self, name: &str) -> Arc<Path> {
        let path: Arc<Path> = self.dir.path().join(name).into();
        if !path.exists() {
            std::fs::write(&path, "{}\n").unwrap();
        }
        let picker = self.compositor.find::<Overlay<Picker<(), ()>>>().unwrap();
        picker.content.preview.get::<(), ()>(
            &self.app.editor,
            (PathOrId::Path(&path), None),
            &self.jobs,
            None,
            Size::new(30, 20),
        );
        path
    }

    fn document(&mut self, path: &Path) -> &Document {
        let CachedPreview::Document(doc) = &self.picker().preview.preview_cache[path].preview
        else {
            panic!("expected a document preview");
        };
        doc
    }

    fn read_preview(&mut self, path: &Path) -> Option<String> {
        let picker = self.compositor.find::<Overlay<Picker<(), ()>>>().unwrap();
        let (preview, _) = picker.content.preview.get::<(), ()>(
            &self.app.editor,
            (PathOrId::Path(path), None),
            &self.jobs,
            None,
            Size::new(30, 20),
        )?;
        preview.document().map(|doc| doc.text().to_string())
    }

    async fn completion(&mut self) -> Callback {
        tokio::time::timeout(Duration::from_secs(10), self.jobs.callbacks.recv())
            .await
            .expect("preview syntax did not finish")
            .expect("preview callback queue closed")
    }

    async fn completion_while_time_paused(&mut self) -> Callback {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(callback) = self.jobs.callbacks.try_recv() {
                return callback;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "preview syntax did not finish"
            );
            // Keep a task runnable while the blocking parser finishes. Awaiting
            // recv would let Tokio advance the paused clock to its timeout.
            tokio::task::yield_now().await;
        }
    }

    fn publish(&mut self, callback: Callback) {
        assert!(self
            .jobs
            .handle_callback(
                crate::config::Context {
                    current: &crate::config::Config::default(),
                    updates: &tokio::sync::mpsc::unbounded_channel().0
                },
                &mut self.app.editor,
                &mut self.compositor,
                Ok(Some(callback))
            )
            .is_none());
    }
}

#[tokio::test]
async fn previews_follow_unwatched_rewrites_deletion_and_recreation() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let path = fixture.dir.path().join("file.txt");
    std::fs::write(&path, "before\n")?;
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("before\n"));
    std::fs::write(&path, "after rewrite\n")?;
    assert_eq!(
        fixture.read_preview(&path).as_deref(),
        Some("after rewrite\n")
    );
    std::fs::remove_file(&path)?;
    assert!(fixture.read_preview(&path).is_none());
    std::fs::write(&path, "recreated\n")?;
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("recreated\n"));
    Ok(())
}

#[tokio::test]
async fn native_batches_invalidate_same_stamp_previews_and_lagged_subscribers() -> anyhow::Result<()>
{
    use view::file_watcher::{CanonicalPathBuf, Event, EventType, Events};

    let mut fixture = Fixture::new()?;
    let path = fixture.dir.path().join("file.txt");
    std::fs::write(&path, "before\n")?;
    let modified = path.metadata()?.modified()?;
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("before\n"));
    std::fs::write(&path, "after!\n")?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_modified(modified)?;
    let events = Events::from(vec![Event {
        path: CanonicalPathBuf::assert_canonicalized(&path.canonicalize()?),
        ty: EventType::Modified,
    }]);
    fixture.app.editor.handle_file_events(&events);
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("after!\n"));

    std::fs::write(&path, "latest\n")?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_modified(modified)?;
    for _ in 0..100 {
        fixture.app.editor.handle_file_events(&events);
    }
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("latest\n"));
    Ok(())
}

#[tokio::test]
async fn directory_previews_follow_unwatched_membership_changes() -> anyhow::Result<()> {
    let mut fixture = Fixture::with_config(|config| {
        config.editor.files.reload.poll.interval = 100;
    })?;
    tokio::time::pause();
    let path = fixture.dir.path().to_owned();
    fixture.read_preview(&path);
    let child = path.join("new.txt");
    std::fs::write(&child, "new\n")?;
    // Directory metadata may not change immediately on every filesystem.
    // Allow the polling backstop to refresh membership without a real sleep.
    tokio::time::advance(Duration::from_millis(100)).await;
    fixture.read_preview(&path);
    let CachedPreview::Directory(entries) = &fixture.picker().preview.preview_cache[&*path].preview
    else {
        panic!("expected directory preview");
    };
    assert!(entries.iter().any(|(path, _)| path == &child));
    std::fs::remove_file(&child)?;
    tokio::time::advance(Duration::from_millis(100)).await;
    fixture.read_preview(&path);
    let CachedPreview::Directory(entries) = &fixture.picker().preview.preview_cache[&*path].preview
    else {
        panic!("expected directory preview");
    };
    assert!(!entries.iter().any(|(path, _)| path == &child));
    Ok(())
}

#[tokio::test]
async fn preview_keeps_the_open_buffer_authoritative() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let path = stdx::path::canonicalize(fixture.dir.path().join("buffer.txt"));
    std::fs::write(&path, "buffer\n")?;
    fixture.app.editor.open(&path, Action::Load)?;
    std::fs::write(&path, "external change\n")?;
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("buffer\n"));
    Ok(())
}

#[tokio::test]
async fn relative_path_previews_follow_unsaved_buffers() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let dir = tempfile::tempdir_in(stdx::env::current_working_dir())?;
    let path = stdx::path::canonicalize(dir.path().join("buffer.txt"));
    let relative = stdx::path::get_relative_path(&path);
    assert!(relative.is_relative());
    std::fs::write(&path, "old text\n")?;
    assert_eq!(
        fixture.read_preview(&relative).as_deref(),
        Some("old text\n")
    );

    let id = fixture.app.editor.open(&path, Action::Replace)?;
    for text in ["new text\n", "another unsaved change\n"] {
        let (view, doc) = view::current!(fixture.app.editor);
        let change = editor_core::Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        view::editing::apply(doc, &change, view.id)?;
        assert!(doc.is_modified());
        assert_eq!(fixture.read_preview(&relative).as_deref(), Some(text));
        assert_eq!(std::fs::read_to_string(&path)?, "old text\n");
    }

    assert!(fixture.app.editor.close_document(id, true).is_ok());
    assert_eq!(
        fixture.read_preview(&relative).as_deref(),
        Some("old text\n")
    );
    Ok(())
}

#[tokio::test]
async fn normalized_path_previews_use_open_buffers_when_files_are_missing() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let path = stdx::path::canonicalize(fixture.dir.path().join("buffer.txt"));
    std::fs::write(&path, "buffer\n")?;
    fixture.app.editor.open(&path, Action::Load)?;
    std::fs::remove_file(&path)?;

    let path = path
        .parent()
        .unwrap()
        .join("missing")
        .join("..")
        .join("buffer.txt");
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("buffer\n"));
    Ok(())
}

#[tokio::test]
async fn directory_polling_refreshes_ignore_rules_without_directory_metadata_changes(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::with_config(|config| {
        config.editor.file_explorer.ignore = true;
        config.editor.files.reload.poll.interval = 100;
    })?;
    let root = fixture.dir.path().to_owned();
    let child = root.join("file.txt");
    std::fs::write(&child, "file")?;
    std::fs::write(root.join(".ignore"), "")?;
    fixture.read_preview(&root);
    let CachedPreview::Directory(entries) = &fixture.picker().preview.preview_cache[&*root].preview
    else {
        panic!("expected directory");
    };
    assert!(entries.iter().any(|(path, _)| path == &child));
    std::fs::write(root.join(".ignore"), "file.txt\n")?;
    tokio::time::sleep(Duration::from_millis(110)).await;
    fixture.read_preview(&root);
    let CachedPreview::Directory(entries) = &fixture.picker().preview.preview_cache[&*root].preview
    else {
        panic!("expected directory");
    };
    assert!(!entries.iter().any(|(path, _)| path == &child));
    Ok(())
}

#[tokio::test]
async fn preview_rendering_tolerates_locations_beyond_a_shortened_file() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let path = fixture.preview("short.json");
    let mut entry = fixture
        .picker()
        .preview
        .preview_cache
        .remove(&path)
        .unwrap();
    let config = Config::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let cx = Context {
        config: crate::config::Context {
            current: &config,
            updates: &updates,
        },
        editor: &mut fixture.app.editor,
        jobs: &mut fixture.jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    let area = Rect::new(0, 0, 30, 20);
    render_preview_content(
        Preview::Cached(&mut entry.preview),
        Some((100, 150)),
        &mut FileScroll::default(),
        area,
        &mut Surface::empty(area),
        &cx,
    );
    Ok(())
}

#[tokio::test]
async fn preview_highlights_after_50_ms_without_restarting_or_switching_queues(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    let path = fixture.preview("preview.json");
    tokio::task::yield_now().await;
    let mut other_jobs = Jobs::new();

    tokio::time::advance(Duration::from_millis(49)).await;
    assert!(fixture.jobs.callbacks.try_recv().is_err());
    // Redrawing the same selection must not start another debounce or request.
    fixture.preview("preview.json");
    tokio::time::advance(Duration::from_millis(2)).await;
    // Keep the clock paused while CPU work finishes so an older, longer debounce
    // would advance it beyond this deadline before delivering a callback.
    let callback = fixture.completion_while_time_paused().await;
    assert_eq!(start.elapsed(), Duration::from_millis(51));
    tokio::time::resume();
    assert!(other_jobs.callbacks.try_recv().is_err());
    fixture.publish(callback);
    assert!(fixture.document(&path).syntax().is_some());
    assert!(!fixture.document(&path).is_syntax_pending());
    Ok(())
}

#[tokio::test]
async fn changing_selection_cancels_the_previous_preview_and_allows_returning() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new()?;
    tokio::time::pause();
    let first = fixture.preview("first.json");
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(30)).await;
    let second = fixture.preview("second.json");
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(51)).await;
    tokio::time::resume();

    let callback = fixture.completion().await;
    fixture.publish(callback);
    assert!(fixture.document(&first).syntax().is_none());
    assert!(fixture.document(&second).syntax().is_some());
    assert!(fixture.jobs.callbacks.try_recv().is_err());

    fixture.preview("first.json");
    let callback = fixture.completion().await;
    fixture.publish(callback);
    assert!(fixture.document(&first).syntax().is_some());
    Ok(())
}

#[tokio::test]
async fn queued_results_cannot_publish_to_hidden_or_replaced_previews() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let path = fixture.preview("preview.json");
    let callback = fixture.completion().await;
    let closed = fixture.app.editor.new_file(Action::Load);
    assert!(fixture.app.editor.close_document(closed, true).is_ok());
    let picker = fixture
        .compositor
        .find::<Overlay<Picker<(), ()>>>()
        .unwrap();
    assert!(picker
        .content
        .preview
        .get::<(), ()>(
            &fixture.app.editor,
            (PathOrId::Id(closed), None),
            &fixture.jobs,
            None,
            Size::new(30, 20),
        )
        .is_none());
    fixture.publish(callback);
    assert!(fixture.document(&path).syntax().is_none());

    fixture.preview("preview.json");
    let callback = fixture.completion().await;
    fixture.picker().preview.clear();
    fixture.preview("preview.json");
    fixture.publish(callback);
    assert!(fixture.document(&path).syntax().is_none());
    assert!(fixture.document(&path).is_syntax_pending());

    let callback = fixture.completion().await;
    fixture.publish(callback);
    assert!(fixture.document(&path).syntax().is_some());
    Ok(())
}

#[tokio::test]
async fn cached_previews_refresh_language_resources_and_ignore_old_results() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let path = fixture.preview("file.json");
    let old_completion = fixture.completion().await;
    let loader = syntax::Loader::new(
        toml::from_str(
            r#"
            [[language]]
            name = "text"
            scope = "text.plain"
            file-types = []
            [[language]]
            name = "json"
            scope = "source.json"
            file-types = ["json"]
            "#,
        )?,
        loader::syntax::Resources::default(),
    )?;
    let expected = loader.language_for_name("json").unwrap();
    fixture
        .app
        .editor
        .apply_language_config(loader, fixture.app.editor.theme.clone())?;

    fixture.preview("file.json");
    fixture.publish(old_completion);
    assert!(fixture.document(&path).is_syntax_pending());
    let completion = fixture.completion().await;
    fixture.publish(completion);
    assert_eq!(
        fixture.document(&path).syntax().unwrap().root_language(),
        expected
    );
    Ok(())
}

#[tokio::test]
async fn replacement_preserves_highlighted_preview() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let path = fixture.preview("preview.json");
    let callback = fixture.completion().await;
    fixture.publish(callback);
    assert!(fixture.document(&path).syntax().is_some());
    let mut controller = event::TaskController::new();
    let selected = search::Search::new(r"\{", true)?
        .search_document(&path, &editor_core::Rope::from("{}\n"))?;
    let request = view::editor::WorkspaceReplacement::new(
        &fixture.app.editor,
        search::Search::new(r"\{", true)?,
        &[&selected[0]],
        "{ ",
        false,
        controller.restart(),
    )?;
    let pending = request.prepare().await?.apply(&mut fixture.app.editor)?;
    // Atomic saves may temporarily remove the file. Keep the last complete,
    // highlighted preview even if native events overflow while writes run.
    let resolved = path.canonicalize()?;
    let backup = path.with_extension("backup");
    std::fs::rename(&path, &backup)?;
    let events = view::file_watcher::Events::from(vec![view::file_watcher::Event {
        path: view::file_watcher::CanonicalPathBuf::assert_canonicalized(&resolved),
        ty: view::file_watcher::EventType::Modified,
    }]);
    for _ in 0..100 {
        fixture.app.editor.handle_file_events(&events);
    }
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("{}\n"));
    assert!(fixture.document(&path).syntax().is_some());
    std::fs::rename(&backup, &path)?;
    let outcome = pending.finish().await;
    assert!(outcome.error.is_none());
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("{ }\n"));
    assert!(fixture.document(&path).syntax().is_some());
    assert!(!fixture.document(&path).is_syntax_pending());
    // Delayed notifications for our own write must not drop highlighting either.
    fixture.app.editor.handle_file_events(&events);
    assert_eq!(fixture.read_preview(&path).as_deref(), Some("{ }\n"));
    assert!(fixture.document(&path).syntax().is_some());
    Ok(())
}
