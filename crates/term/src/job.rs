use arc_swap::ArcSwapOption;
use event::status::StatusMessage;
use event::{runtime_local, send_blocking};
use std::{collections::VecDeque, sync::Arc};
use view::callbacks::{EditorCallback, EditorCallbackSender};
use view::Editor;

use crate::compositor::Compositor;

use futures_util::future::{BoxFuture, Future, FutureExt};
use futures_util::stream::FuturesUnordered;
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub type EditorCompositorCallback = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;
pub type EditorCallbackFollowup = Box<dyn FnOnce(&mut Editor) -> Option<Job> + Send>;

runtime_local! {
    static JOB_QUEUE: ArcSwapOption<Sender<Callback>> = ArcSwapOption::const_empty();
}

fn callback_sender() -> Arc<Sender<Callback>> {
    JOB_QUEUE.load_full().expect("job queue is not initialized")
}

pub async fn dispatch_callback(job: Callback) {
    let _ = callback_sender().send(job).await;
}

pub async fn dispatch(job: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static) {
    let _ = callback_sender()
        .send(Callback::EditorCompositor(Box::new(job)))
        .await;
}

pub fn dispatch_blocking(job: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static) {
    let jobs = callback_sender();
    send_blocking(&jobs, Callback::EditorCompositor(Box::new(job)))
}

pub enum Callback {
    EditorCompositor(EditorCompositorCallback),
    Editor(EditorCallback),
    Followup(EditorCallbackFollowup),
    TryEditor(AfterWrites),
}

pub type AfterWrites = Box<dyn FnOnce(&mut Editor) -> anyhow::Result<()> + Send>;

pub type JobFuture = BoxFuture<'static, anyhow::Result<Option<Callback>>>;

pub struct Job {
    pub future: BoxFuture<'static, anyhow::Result<Option<Callback>>>,
    /// Do we need to wait for this job to finish before exiting?
    pub wait: bool,
}

pub struct Jobs {
    sender: Arc<Sender<Callback>>,
    /// jobs that need to complete before we exit.
    pub wait_futures: FuturesUnordered<JobFuture>,
    pub callbacks: Receiver<Callback>,
    pub status_messages: Receiver<StatusMessage>,
    after_writes: VecDeque<AfterWrites>,
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
        let sender = Arc::new(tx);
        let status_messages = event::status::setup();
        Self {
            sender,
            wait_futures: FuturesUnordered::new(),
            callbacks: rx,
            status_messages,
            after_writes: VecDeque::new(),
        }
    }

    /// Use this editor's queue for callbacks dispatched by event hooks.
    /// Creating another job collection does not replace this queue.
    pub(crate) fn set_current(&self) {
        JOB_QUEUE.store(Some(self.sender.clone()));
    }

    /// Bind editor-only completions to this queue, independent of global dispatch.
    pub fn editor_callback_sender(&self) -> EditorCallbackSender {
        let sender = self.sender.clone();
        let blocking_sender = sender.clone();
        EditorCallbackSender::new(
            move |callback| {
                let sender = sender.clone();
                async move {
                    let _ = sender.send(Callback::Editor(callback)).await;
                }
            },
            move |callback| send_blocking(&blocking_sender, Callback::Editor(callback)),
        )
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
        editor: &mut Editor,
        compositor: &mut Compositor,
        call: anyhow::Result<Option<Callback>>,
    ) -> Option<Job> {
        match call {
            Ok(None) => None,
            Ok(Some(call)) => match call {
                Callback::EditorCompositor(call) => {
                    call(editor, compositor);
                    None
                }
                Callback::Editor(call) => {
                    call(editor);
                    None
                }
                Callback::Followup(call) => call(editor),
                Callback::TryEditor(call) => {
                    if let Err(err) = call(editor) {
                        self.after_writes.clear();
                        editor.set_error(|| err.to_string());
                    }
                    None
                }
            },
            Err(e) => {
                self.after_writes.clear();
                editor.set_error(|| format!("Async job failed: {}", e));
                None
            }
        }
    }

    pub fn add(&self, j: Job) {
        if j.wait {
            self.wait_futures.push(j.future);
        } else {
            // Keep each job attached to its originating editor, even if a new
            // editor replaces the runtime's default dispatch queue meanwhile.
            let sender = self.sender.clone();
            tokio::spawn(async move {
                match j.future.await {
                    Ok(Some(cb)) => {
                        let _ = sender.send(cb).await;
                    }
                    Ok(None) => (),
                    Err(err) => event::status::report(err).await,
                }
            });
        }
    }

    /// Defer lifecycle operations without blocking LSP requests or UI events.
    pub fn after_writes(&mut self, editor: &mut Editor, action: AfterWrites) -> anyhow::Result<()> {
        if self.wait_futures.is_empty() && editor.write_count == 0 {
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
        while self.wait_futures.is_empty() && editor.write_count == 0 {
            let Some(action) = self.after_writes.pop_front() else {
                break;
            };
            if let Err(err) = action(editor) {
                self.after_writes.clear();
                editor.set_error(|| err.to_string());
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Without the integration-test feature, the selected queue is process-global.
    static QUEUE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn editor_sender_keeps_its_queue_and_backpressure() {
        let _guard = QUEUE_TEST_LOCK.lock().await;
        let mut first = Jobs::new();
        let sender = first.editor_callback_sender();
        let mut second = Jobs::new();
        second.set_current();

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
    async fn callbacks_follow_the_selected_queue_and_jobs_keep_their_owner() {
        let _guard = QUEUE_TEST_LOCK.lock().await;
        let mut first = Jobs::new();
        first.set_current();
        let mut second = Jobs::new();

        // Creating a temporary collection must not redirect event-hook callbacks.
        dispatch_callback(Callback::Editor(Box::new(|_| {}))).await;
        assert!(first.callbacks.try_recv().is_ok());
        assert!(second.callbacks.try_recv().is_err());

        second.set_current();
        first.callback(async { Ok(Callback::Editor(Box::new(|_| {}))) });
        tokio::time::timeout(std::time::Duration::from_secs(1), first.callbacks.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(second.callbacks.try_recv().is_err());

        dispatch_callback(Callback::Editor(Box::new(|_| {}))).await;
        assert!(second.callbacks.try_recv().is_ok());
    }
}
