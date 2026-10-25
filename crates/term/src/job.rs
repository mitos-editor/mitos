use event::send_blocking;
use std::collections::VecDeque;
use view::callbacks::{EditorCallback, EditorCallbackSender};
use view::Editor;

use crate::compositor::Compositor;

use futures_util::future::{BoxFuture, Future, FutureExt};
use futures_util::stream::FuturesUnordered;
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub type EditorCompositorCallback = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;
pub type EditorCallbackFollowup =
    Box<dyn FnOnce(&mut Editor) -> anyhow::Result<Option<Job>> + Send>;

/// A bounded callback destination belonging to one application.
#[derive(Clone)]
pub struct JobSender(Sender<Callback>);

impl JobSender {
    pub async fn send_callback(&self, callback: Callback) {
        let _ = self.0.send(callback).await;
    }

    pub async fn send(&self, callback: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static) {
        self.send_callback(Callback::EditorCompositor(Box::new(callback)))
            .await;
    }

    pub fn send_blocking(
        &self,
        callback: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static,
    ) {
        send_blocking(&self.0, Callback::EditorCompositor(Box::new(callback)));
    }
}

pub type ContextCallback = Box<dyn FnOnce(&mut Compositor, &mut crate::compositor::Context) + Send>;

pub enum Callback {
    Context(ContextCallback),
    EditorCompositor(EditorCompositorCallback),
    Editor(EditorCallback),
    Followup(EditorCallbackFollowup),
}

pub type AfterWrites = Box<dyn FnOnce(&mut Editor) -> anyhow::Result<()> + Send>;

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

pub struct Jobs {
    sender: JobSender,
    editor_callbacks: EditorCallbackSender,
    pub(crate) command_expansion: event::TaskController,
    /// jobs that need to complete before we exit.
    pub wait_futures: FuturesUnordered<JobFuture>,
    pub callbacks: Receiver<Callback>,
    after_writes: VecDeque<AfterWrites>,
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
            sender,
            editor_callbacks,
            command_expansion: event::TaskController::new(),
            wait_futures: FuturesUnordered::new(),
            callbacks: rx,
            after_writes: VecDeque::new(),
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

    pub fn handle_callback(
        &mut self,
        config: crate::config::Context<'_>,
        editor: &mut Editor,
        compositor: &mut Compositor,
        call: anyhow::Result<Option<Callback>>,
    ) -> Option<Job> {
        match self.try_handle_callback(config, editor, compositor, call) {
            Ok(job) => job,
            Err(error) => {
                self.cancel_after_writes();
                editor.set_error(|| format!("Async job failed: {error}"));
                None
            }
        }
    }

    /// Apply a completion, exposing errors to shutdown as well as the event loop.
    pub fn try_handle_callback(
        &mut self,
        config: crate::config::Context<'_>,
        editor: &mut Editor,
        compositor: &mut Compositor,
        call: anyhow::Result<Option<Callback>>,
    ) -> anyhow::Result<Option<Job>> {
        if let Some(call) = call? {
            match call {
                Callback::Context(call) => call(
                    compositor,
                    &mut crate::compositor::Context {
                        config,
                        editor,
                        jobs: self,
                        scroll: None,
                        image_picker: None,
                        is_cursor_owner: false,
                    },
                ),
                Callback::EditorCompositor(call) => call(editor, compositor),
                Callback::Editor(call) => call(editor),
                Callback::Followup(call) => return call(editor),
            }
        }
        Ok(None)
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
                            .send_callback(Callback::Editor(Box::new(move |editor| {
                                editor.set_error(|| err.to_string());
                            })))
                            .await;
                    }
                }
            });
        }
    }

    /// Defer lifecycle operations without blocking LSP requests or UI events.
    pub fn after_writes(&mut self, editor: &mut Editor, action: AfterWrites) -> anyhow::Result<()> {
        if self.wait_futures.is_empty() && !editor.has_pending_writes() {
            action(editor)
        } else {
            self.after_writes.push_back(action);
            Ok(())
        }
    }

    pub fn cancel_after_writes(&mut self) {
        self.after_writes.clear();
    }

    pub fn run_after_writes(&mut self, editor: &mut Editor) {
        while self.wait_futures.is_empty() && !editor.has_pending_writes() {
            let Some(action) = self.after_writes.pop_front() else {
                break;
            };
            if let Err(error) = action(editor) {
                self.cancel_after_writes();
                editor.set_error(|| error.to_string());
                break;
            }
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
        sender
            .send_callback(Callback::Editor(Box::new(|_| {})))
            .await;
        assert!(first.callbacks.try_recv().is_ok());
        assert!(second.callbacks.try_recv().is_err());

        first.callback(async { Ok(Callback::Editor(Box::new(|_| {}))) });
        tokio::time::timeout(std::time::Duration::from_secs(1), first.callbacks.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(second.callbacks.try_recv().is_err());

        second
            .sender()
            .send_callback(Callback::Editor(Box::new(|_| {})))
            .await;
        assert!(second.callbacks.try_recv().is_ok());
        drop(first);
        sender.send(|_, _| {}).await;
        sender.send_blocking(|_, _| {});
    }
}
