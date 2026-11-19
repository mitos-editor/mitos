//! Dismissible preparation for a menu or prompt that owns its provider requests.

use std::future::Future;

use event::{TaskController, TaskHandle};
use tokio::sync::oneshot;
use tui::buffer::Buffer;
use view::graphics::Rect;

use crate::{
    compositor::{Component, Compositor, Context, DeferredEvent, DrawContext, Event, EventResult},
    job::Callback,
};

use super::{Popup, PopupBorder, Text};

type Apply<T> = Box<dyn FnOnce(&mut Compositor, &mut Context, T) -> bool + Send>;

pub(crate) struct Pending<T> {
    popup: Popup<Text>,
    _task: TaskController,
    load: TaskHandle,
    result: Option<oneshot::Receiver<T>>,
    apply: Option<Apply<T>>,
    input: Vec<DeferredEvent>,
}

impl<T: Send + 'static> Pending<T> {
    /// `apply` returns true when it installed the UI that should receive early input.
    pub fn new(
        id: &'static str,
        message: &'static str,
        redraw: event::Redraw,
        future: impl Future<Output = T> + Send + 'static,
        apply: impl FnOnce(&mut Compositor, &mut Context, T) -> bool + Send + 'static,
    ) -> Self {
        let mut task = TaskController::new();
        let load = task.restart();
        let cancel = load.clone();
        let (sender, result) = oneshot::channel();
        tokio::spawn(async move {
            let _redraw = redraw.on_drop();
            if let Some(result) = event::cancelable_future(future, cancel).await {
                let _ = sender.send(result);
            }
        });
        Self {
            popup: Popup::new(id, Text::new(message.into())).auto_close(true),
            _task: task,
            load,
            result: Some(result),
            apply: Some(Box::new(apply)),
            input: Vec::new(),
        }
    }

    pub fn with_border(mut self, border: PopupBorder) -> Self {
        self.popup = self.popup.with_border(border);
        self
    }
}

impl<T: Send + 'static> Component for Pending<T> {
    fn update(&mut self, area: Rect, cx: &mut Context) {
        self.popup.update(area, cx);
        let Some(result) = &mut self.result else {
            return;
        };
        let result = match result.try_recv() {
            Ok(result) => Some(result),
            Err(oneshot::error::TryRecvError::Closed) => None,
            Err(oneshot::error::TryRecvError::Empty) => return,
        };
        self.result = None;
        let apply = self.apply.take().unwrap();
        let load = self.load.clone();
        let id = self.popup.id().unwrap();
        cx.jobs
            .sender()
            .enqueue(Callback::ui(move |compositor, cx| {
                if load.is_canceled() {
                    return;
                }
                let Some(pending) = compositor.find_id::<Self>(id) else {
                    return;
                };
                // Input can arrive after this callback was queued. Take it only now.
                let input = std::mem::take(&mut pending.input);
                compositor.remove(id);
                if let Some(result) = result
                    && apply(compositor, cx, result)
                {
                    compositor.replay_deferred(input, cx);
                }
            }));
    }

    fn has_pending_keys(&self) -> bool {
        self.popup.has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        match self.popup.handle_event(event, cx) {
            EventResult::Consumed(Some(callback)) => EventResult::Consumed(Some(callback)),
            EventResult::Consumed(None) | EventResult::Ignored(_)
                if matches!(event, Event::Key(_) | Event::Paste(_)) =>
            {
                self.input
                    .push(DeferredEvent::capture(event, cx.interaction));
                EventResult::Consumed(None)
            }
            result => result,
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut DrawContext) {
        self.popup.render(area, surface, cx);
    }

    fn owns_cursor(&self) -> bool {
        true
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.popup.required_size(viewport)
    }

    fn id(&self) -> Option<&'static str> {
        self.popup.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };

    use crate::{application::TestApplication, config::Config, job::Jobs};

    const ID: &str = "pending-test";
    const AREA: Rect = Rect::new(0, 0, 80, 24);

    struct Recorder(Arc<Mutex<Vec<Event>>>);

    impl Component for Recorder {
        fn handle_event(&mut self, event: &Event, _: &mut Context) -> EventResult {
            self.0.lock().unwrap().push(event.clone());
            EventResult::Consumed(None)
        }

        fn render(&mut self, _: Rect, _: &mut Buffer, _: &mut DrawContext) {}
    }

    struct Fixture {
        app: TestApplication,
        compositor: Compositor,
        jobs: Jobs,
        config: Config,
        updates: tokio::sync::mpsc::UnboundedSender<crate::config::ConfigEvent>,
        input: Arc<Mutex<Vec<Event>>>,
    }

    impl Fixture {
        fn new() -> anyhow::Result<Self> {
            let input = Arc::new(Mutex::new(Vec::new()));
            let mut compositor = Compositor::new(AREA);
            compositor.push(Box::new(Recorder(input.clone())));
            Ok(Self {
                app: crate::ui::picker::test::application(false)?,
                compositor,
                jobs: Jobs::new(),
                config: Config::default(),
                updates: tokio::sync::mpsc::unbounded_channel().0,
                input,
            })
        }

        fn run<R>(&mut self, call: impl FnOnce(&mut Compositor, &mut Context) -> R) -> R {
            call(
                &mut self.compositor,
                &mut Context {
                    config: crate::config::Context {
                        current: &self.config,
                        updates: &self.updates,
                    },
                    editor: &mut self.app.editor,
                    interaction: &mut self.app.interaction,
                    jobs: &mut self.jobs,
                    scroll: None,
                    image_picker: None,
                    is_cursor_owner: true,
                },
            )
        }

        fn event(&mut self, event: Event) {
            self.run(|compositor, cx| compositor.handle_event(&event, cx));
        }

        async fn completion(&mut self) -> Callback {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    self.run(|compositor, cx| {
                        compositor
                            .find_id::<Pending<()>>(ID)
                            .unwrap()
                            .update(AREA, cx)
                    });
                    if let Ok(callback) = self.jobs.callbacks.try_recv() {
                        return callback;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap()
        }

        fn apply(&mut self, callback: Callback) {
            self.jobs.handle_callback(
                crate::config::Context {
                    current: &self.config,
                    updates: &self.updates,
                },
                &mut self.app.editor,
                &mut self.app.interaction,
                &mut self.compositor,
                Ok(Some(callback)),
            );
        }
    }

    fn key(key: &str) -> Event {
        Event::Key(key.parse().unwrap())
    }

    #[tokio::test]
    async fn dropping_preparation_releases_its_provider_future() {
        struct Finished(Option<oneshot::Sender<()>>);
        impl Drop for Finished {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let (started, ready) = oneshot::channel();
        let (finished, done) = oneshot::channel();
        let pending = Pending::new(
            ID,
            "Loading",
            event::Redraw::default(),
            async move {
                let _finished = Finished(Some(finished));
                let _ = started.send(());
                std::future::pending::<()>().await;
            },
            |_, _, ()| false,
        );
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        drop(pending);
        tokio::time::timeout(Duration::from_secs(5), done)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn completed_preparation_replays_early_and_later_input_only_to_its_new_ui(
    ) -> anyhow::Result<()> {
        let mut fixture = Fixture::new()?;
        let input = fixture.input.clone();
        fixture.compositor.push(Box::new(Pending::new(
            ID,
            "Loading",
            fixture.app.editor.redraw().clone(),
            async {},
            move |compositor, _, ()| {
                compositor.push(Box::new(Recorder(input)));
                true
            },
        )));
        fixture.event(key("x"));
        // Popup scrolling binds C-u, but early editing keys belong to the eventual prompt.
        fixture.event(key("C-u"));
        assert!(fixture.input.lock().unwrap().is_empty());
        let callback = fixture.completion().await;
        fixture.event(Event::Paste("tail".into()));
        assert!(fixture.input.lock().unwrap().is_empty());
        fixture.apply(callback);
        {
            let input = fixture.input.lock().unwrap();
            assert_eq!(input.len(), 3);
            assert!(matches!(&input[0], Event::Key(key) if *key == "x".parse().unwrap()));
            assert!(matches!(&input[1], Event::Key(key) if *key == "C-u".parse().unwrap()));
            assert!(matches!(&input[2], Event::Paste(text) if text == "tail"));
        }
        assert!(fixture.app.close().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn queued_preparation_cannot_publish_after_dismissal_or_replacement() -> anyhow::Result<()>
    {
        for replace in [false, true] {
            let mut fixture = Fixture::new()?;
            let published = Arc::new(AtomicBool::new(false));
            let apply = published.clone();
            fixture.compositor.push(Box::new(Pending::new(
                ID,
                "Loading",
                fixture.app.editor.redraw().clone(),
                async {},
                move |_, _, ()| {
                    apply.store(true, Ordering::Relaxed);
                    true
                },
            )));
            fixture.event(key("x"));
            let callback = fixture.completion().await;
            if replace {
                fixture.compositor.replace_or_push(
                    ID,
                    Pending::new(
                        ID,
                        "Another load",
                        fixture.app.editor.redraw().clone(),
                        std::future::pending::<()>(),
                        |_, _, ()| false,
                    ),
                );
            } else {
                fixture.event(key("esc"));
            }
            fixture.apply(callback);
            assert!(!published.load(Ordering::Relaxed));
            assert!(fixture.input.lock().unwrap().is_empty());
            fixture.compositor.remove(ID);
            assert!(fixture.app.close().await.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn results_without_a_target_ui_discard_buffered_input() -> anyhow::Result<()> {
        let mut fixture = Fixture::new()?;
        fixture.compositor.push(Box::new(Pending::new(
            ID,
            "Loading",
            fixture.app.editor.redraw().clone(),
            async {},
            |_, _, ()| false,
        )));
        fixture.event(key("x"));
        fixture.event(Event::Paste("tail".into()));
        let callback = fixture.completion().await;
        fixture.apply(callback);
        assert!(fixture.input.lock().unwrap().is_empty());
        assert_eq!(fixture.compositor.layer_count(), 1);
        assert!(fixture.app.close().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn preparation_honors_custom_popup_cancellation_prefixes() -> anyhow::Result<()> {
        let mut fixture = Fixture::new()?;
        fixture.config.component_keys = Config::load(
            Ok(&"[keys.popup]\ng = { q = 'cancel' }\n".to_owned()),
            Err(crate::config::ConfigLoadError::default()),
        )
        .unwrap()
        .component_keys;
        fixture.compositor.push(Box::new(Pending::new(
            ID,
            "Loading",
            fixture.app.editor.redraw().clone(),
            std::future::pending::<()>(),
            |_, _, ()| false,
        )));
        fixture.event(key("g"));
        assert!(fixture
            .compositor
            .find_id::<Pending<()>>(ID)
            .unwrap()
            .has_pending_keys());
        fixture.event(key("q"));
        assert!(fixture.compositor.find_id::<Pending<()>>(ID).is_none());
        assert!(fixture.input.lock().unwrap().is_empty());
        assert!(fixture.app.close().await.is_empty());
        Ok(())
    }
}
