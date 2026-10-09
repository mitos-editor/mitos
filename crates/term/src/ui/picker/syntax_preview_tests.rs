use std::time::Duration;

use editor_core::syntax;

use super::*;
use crate::{
    application::Application,
    args::Args,
    config::Config,
    job::{Callback, Jobs},
    ui::overlay::{overlaid, Overlay},
};

struct Fixture {
    app: Application,
    jobs: Jobs,
    compositor: Compositor,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> anyhow::Result<Self> {
        let mut config = Config::default();
        config.editor.lsp.enable = false;
        config.editor.file_watcher.enable = false;
        config.editor.auto_reload.enable = false;
        config.editor.word_completion.enable = false;
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
            loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        )?;
        let picker = Picker::new(
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
        std::fs::write(&path, "{}\n").unwrap();
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
        let CachedPreview::Document(doc) = &self.picker().preview.preview_cache[path] else {
            panic!("expected a document preview");
        };
        doc
    }

    async fn completion(&mut self) -> Callback {
        tokio::time::timeout(Duration::from_secs(10), self.jobs.callbacks.recv())
            .await
            .expect("preview syntax did not finish")
            .expect("preview callback queue closed")
    }

    fn publish(&mut self, callback: Callback) {
        assert!(self
            .jobs
            .handle_callback(
                &mut self.app.editor,
                &mut self.compositor,
                Ok(Some(callback))
            )
            .is_none());
    }
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
    other_jobs.set_current();

    tokio::time::advance(Duration::from_millis(49)).await;
    assert!(fixture.jobs.callbacks.try_recv().is_err());
    // Redrawing the same selection must not start another debounce or request.
    fixture.preview("preview.json");
    tokio::time::advance(Duration::from_millis(2)).await;
    // Keep the clock paused while CPU work finishes so an older, longer debounce
    // would advance it beyond this deadline before delivering a callback.
    let callback = fixture.completion().await;
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
