use event::send_blocking;
use view::callbacks::{CallbackOutcome, EditorCallback, EditorCallbackSender};
use view::Editor;

use crate::compositor::{Compositor, InteractionState};

use futures_util::future::{BoxFuture, Future, FutureExt};
use futures_util::stream::FuturesUnordered;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// A bounded callback destination belonging to one application.
#[derive(Clone)]
pub struct JobSender(Sender<Callback>);

impl JobSender {
    /// Queue ready work now; retain delivery under the existing channel backpressure.
    pub(crate) fn enqueue(&self, callback: Callback) {
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(callback)) =
            self.0.try_send(callback)
        {
            let sender = self.clone();
            tokio::spawn(async move { sender.send_callback(callback).await });
        }
    }

    pub async fn send_callback(&self, callback: Callback) {
        let _ = self.0.send(callback).await;
    }

    pub async fn send(
        &self,
        callback: impl FnOnce(&mut Compositor, &mut crate::compositor::Context) + Send + 'static,
    ) {
        self.send_callback(Callback::ui(callback)).await;
    }

    pub fn send_blocking(
        &self,
        callback: impl FnOnce(&mut Compositor, &mut crate::compositor::Context) + Send + 'static,
    ) {
        send_blocking(&self.0, Callback::ui(callback));
    }
}

type CallbackResult = anyhow::Result<CallbackOutcome>;
type UiCallback =
    Box<dyn FnOnce(&mut Compositor, &mut crate::compositor::Context) -> CallbackResult + Send>;

pub enum Callback {
    Editor(EditorCallback),
    Ui(UiCallback),
}

impl Callback {
    pub fn editor<R: Into<CallbackOutcome>>(
        callback: impl FnOnce(&mut Editor) -> R + Send + 'static,
    ) -> Self {
        Self::Editor(Box::new(move |editor| callback(editor).into()))
    }

    pub fn ui<R: Into<CallbackOutcome>>(
        callback: impl FnOnce(&mut Compositor, &mut crate::compositor::Context) -> R + Send + 'static,
    ) -> Self {
        Self::Ui(Box::new(move |compositor, cx| {
            Ok(callback(compositor, cx).into())
        }))
    }
}

pub type JobFuture = BoxFuture<'static, anyhow::Result<Option<Callback>>>;

pub struct Job {
    pub future: BoxFuture<'static, anyhow::Result<Option<Callback>>>,
    /// Do we need to wait for this job to finish before exiting?
    pub wait: bool,
}

pub(crate) struct InteractiveShellRequest {
    pub command: tokio::process::Command,
    pub result: tokio::sync::oneshot::Sender<anyhow::Result<std::process::Output>>,
}

#[derive(Default)]
pub(crate) struct SmoothScroll {
    pub(crate) task: event::TaskController,
    pub(crate) view: Option<view::ViewId>,
    pub(crate) remaining: isize,
}

pub struct Jobs {
    pub(crate) image_cache: crate::ui::image::ImageCache,
    sender: JobSender,
    editor_callbacks: EditorCallbackSender,
    pub(crate) command_expansion: event::TaskController,
    pub(crate) smooth_scroll: SmoothScroll,
    /// jobs that need to complete before we exit.
    pub wait_futures: FuturesUnordered<JobFuture>,
    pub callbacks: Receiver<Callback>,
    interactive_executor: view::expansion::InteractiveShell,
    pub(crate) interactive_requests: Receiver<InteractiveShellRequest>,
}

impl Job {
    pub fn new<F: Future<Output = anyhow::Result<()>> + Send + 'static>(f: F) -> Self {
        Self {
            future: f.map(|r| r.map(|()| None)).boxed(),
            wait: false,
        }
    }

    pub fn with_callback<F: Future<Output = anyhow::Result<Callback>> + Send + 'static>(
        f: F,
    ) -> Self {
        Self {
            future: f.map(|r| r.map(Some)).boxed(),
            wait: false,
        }
    }

    pub fn wait_before_exiting(mut self) -> Self {
        self.wait = true;
        self
    }
}

impl Jobs {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (tx, rx) = channel(1024);
        let sender = JobSender(tx);
        let editor_sender = sender.0.clone();
        let blocking_sender = editor_sender.clone();
        let editor_callbacks = EditorCallbackSender::new(
            event::Redraw::default(),
            move |callback| {
                let sender = editor_sender.clone();
                async move {
                    let _ = sender.send(Callback::Editor(callback)).await;
                }
            },
            move |callback| send_blocking(&blocking_sender, Callback::Editor(callback)),
        );
        let (interactive_sender, interactive_requests) = channel(1);
        Self {
            image_cache: Default::default(),
            sender,
            editor_callbacks,
            command_expansion: event::TaskController::new(),
            smooth_scroll: SmoothScroll::default(),
            wait_futures: FuturesUnordered::new(),
            callbacks: rx,
            interactive_executor: interactive_executor(interactive_sender),
            interactive_requests,
        }
    }

    pub(crate) fn interactive_shell(&self) -> view::expansion::InteractiveShell {
        self.interactive_executor.clone()
    }

    pub fn sender(&self) -> JobSender {
        self.sender.clone()
    }

    /// Clone the editor destination bound to this application's queue at construction.
    pub fn editor_callback_sender(&self) -> EditorCallbackSender {
        self.editor_callbacks.clone()
    }

    pub fn spawn<F: Future<Output = anyhow::Result<()>> + Send + 'static>(&mut self, f: F) {
        self.add(Job::new(f));
    }

    pub fn callback<F: Future<Output = anyhow::Result<Callback>> + Send + 'static>(
        &mut self,
        f: F,
    ) {
        self.add(Job::with_callback(f));
    }

    pub(crate) fn debugger_callback<T: Send + 'static>(
        &mut self,
        controller: &mut event::TaskController,
        task: view::debugger::DebuggerTask<T>,
        apply: impl FnOnce(&mut Editor, &mut Compositor, Option<view::debugger::DebuggerUpdate<T>>)
            + Send
            + 'static,
    ) {
        let load = controller.restart();
        self.add(Job {
            wait: false,
            future: Box::pin(async move {
                let update = tokio::select! {
                    biased;
                    _ = load.canceled() => return Ok(None),
                    update = task.run() => update,
                };
                Ok(Some(Callback::ui(move |compositor, cx| {
                    let editor = &mut *cx.editor;
                    if !load.is_canceled() {
                        apply(editor, compositor, update);
                    }
                })))
            }),
        });
    }

    pub fn handle_callback(
        &mut self,
        config: crate::config::Context<'_>,
        editor: &mut Editor,
        interaction: &mut InteractionState,
        compositor: &mut Compositor,
        call: anyhow::Result<Option<Callback>>,
    ) -> CallbackOutcome {
        match self.apply_callback(config, editor, interaction, compositor, call) {
            Ok(result) => result,
            Err(error) => {
                editor.cancel_after_writes();
                editor.set_error(|| format!("Async job failed: {error}"));
                CallbackOutcome::Redraw
            }
        }
    }

    /// Apply a completion, exposing errors to shutdown as well as the event loop.
    pub fn try_handle_callback(
        &mut self,
        config: crate::config::Context<'_>,
        editor: &mut Editor,
        interaction: &mut InteractionState,
        compositor: &mut Compositor,
        call: anyhow::Result<Option<Callback>>,
    ) -> anyhow::Result<()> {
        self.apply_callback(config, editor, interaction, compositor, call)
            .map(|_| ())
    }

    fn apply_callback(
        &mut self,
        config: crate::config::Context<'_>,
        editor: &mut Editor,
        interaction: &mut InteractionState,
        compositor: &mut Compositor,
        call: anyhow::Result<Option<Callback>>,
    ) -> CallbackResult {
        let Some(call) = call? else {
            return Ok(CallbackOutcome::None);
        };
        match call {
            Callback::Editor(call) => Ok(call(editor)),
            Callback::Ui(call) => call(
                compositor,
                &mut crate::compositor::Context {
                    config,
                    editor,
                    interaction,
                    jobs: self,
                    scroll: None,
                    image_picker: None,
                    is_cursor_owner: false,
                },
            ),
        }
    }

    pub fn add(&self, j: Job) {
        if j.wait {
            self.wait_futures.push(j.future);
        } else {
            // Deliver completions to the originating application.
            let sender = self.sender.clone();
            tokio::spawn(async move {
                match j.future.await {
                    Ok(Some(cb)) => {
                        sender.send_callback(cb).await;
                    }
                    Ok(None) => (),
                    Err(err) => {
                        sender
                            .send_callback(Callback::editor(move |editor| {
                                editor.set_error(|| err.to_string());
                            }))
                            .await;
                    }
                }
            });
        }
    }

    /// Frontend-required jobs also delay editor-owned write barriers.
    pub fn after_writes(
        &mut self,
        editor: &mut Editor,
        action: view::save::AfterWrites,
    ) -> anyhow::Result<()> {
        if self.wait_futures.is_empty() {
            editor.after_writes(action)
        } else {
            editor.defer_after_writes(action);
            Ok(())
        }
    }
}

fn interactive_executor(
    sender: Sender<InteractiveShellRequest>,
) -> view::expansion::InteractiveShell {
    std::sync::Arc::new(move |command| {
        let sender = sender.clone();
        async move {
            let (result, receiver) = tokio::sync::oneshot::channel();
            sender
                .send(InteractiveShellRequest { command, result })
                .await
                .map_err(|_| anyhow::anyhow!("editor closed before interactive shell execution"))?;
            receiver
                .await
                .map_err(|_| anyhow::anyhow!("interactive shell execution cancelled"))?
        }
        .boxed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ready_callbacks_enqueue_immediately_and_wait_for_capacity() {
        let (tx, mut rx) = channel(1);
        let sender = JobSender(tx);
        sender.enqueue(Callback::ui(|_, _| {}));
        assert!(matches!(rx.try_recv(), Ok(Callback::Ui(_))));

        sender.enqueue(Callback::editor(|_| {}));
        sender.enqueue(Callback::ui(|_, _| {}));
        assert!(matches!(rx.recv().await, Some(Callback::Editor(_))));
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await,
            Ok(Some(Callback::Ui(_)))
        ));
        drop(rx);
        sender.enqueue(Callback::ui(|_, _| {}));
    }

    #[tokio::test]
    async fn editor_sender_keeps_its_queue_and_backpressure() {
        let mut first = Jobs::new();
        let sender = first.editor_callback_sender();
        let mut second = Jobs::new();

        assert!(sender.same_destination(&first.editor_callback_sender()));
        assert!(!sender.same_destination(&second.editor_callback_sender()));

        sender.send_blocking(|_| {});
        assert!(matches!(
            first.callbacks.try_recv(),
            Ok(Callback::Editor(_))
        ));
        assert!(second.callbacks.try_recv().is_err());

        for _ in 0..1024 {
            sender.send(|_| {}).await;
        }
        let mut pending = std::pin::pin!(sender.send(|_| {}));
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert!(matches!(
            first.callbacks.recv().await,
            Some(Callback::Editor(_))
        ));
        pending.await;
        assert!(second.callbacks.try_recv().is_err());

        // Shutdown drops queued completions and future sends finish without blocking.
        drop(first);
        sender.send(|_| {}).await;
        sender.send_blocking(|_| {});
    }

    #[tokio::test]
    async fn application_senders_and_background_jobs_keep_their_owner() {
        let mut first = Jobs::new();
        let mut second = Jobs::new();

        let sender = first.sender();
        sender.send_callback(Callback::editor(|_| {})).await;
        assert!(first.callbacks.try_recv().is_ok());
        assert!(second.callbacks.try_recv().is_err());

        first.callback(async { Ok(Callback::editor(|_| {})) });
        tokio::time::timeout(std::time::Duration::from_secs(1), first.callbacks.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(second.callbacks.try_recv().is_err());

        second
            .sender()
            .send_callback(Callback::editor(|_| {}))
            .await;
        assert!(second.callbacks.try_recv().is_ok());
        drop(first);
        sender.send(|_, _| {}).await;
        sender.send_blocking(|_, _| {});
    }
}
