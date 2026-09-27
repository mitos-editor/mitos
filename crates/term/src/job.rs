use event::send_blocking;
use std::collections::VecDeque;
use view::callbacks::{EditorCallback, EditorCallbackSender};
use view::Editor;

use crate::compositor::Compositor;

use futures_util::future::{BoxFuture, Future, FutureExt};
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub type EditorCompositorCallback = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;
pub type EditorCallbackFollowup = Box<dyn FnOnce(&mut Editor) -> anyhow::Result<Option<Job>> + Send>;

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

pub struct Jobs {
    sender: JobSender,
    editor_callbacks: EditorCallbackSender,
    pub(crate) command_expansion: event::TaskController,
    /// jobs that need to complete before we exit.
    pub wait_futures: FuturesUnordered<JobFuture>,
    pub callbacks: Receiver<Callback>,
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
        Self {
            sender,
            editor_callbacks,
            command_expansion: event::TaskController::new(),
            wait_futures: FuturesUnordered::new(),
            callbacks: rx,
            after_writes: VecDeque::new(),
        }
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
        match call {
            Ok(None) => None,
            Ok(Some(call)) => match call {
                Callback::Context(call) => {
                    call(
                        compositor,
                        &mut crate::compositor::Context {
                            config,
                            editor,
                            jobs: self,
                            scroll: None,
                            image_picker: None,
                            is_cursor_owner: false,
                        },
                    );
                    None
                }
                Callback::EditorCompositor(call) => {
                    call(editor, compositor);
                    None
                }
                Callback::Editor(call) => {
                    call(editor);
                    None
                }
                Callback::Followup(call) => match call(editor) {
                    Ok(job) => job,
                    Err(error) => {
                        self.cancel_after_writes();
                        editor.set_error(|| format!("Error saving: {error}"));
                        None
                    }
                },
            },
            Err(e) => {
                self.cancel_after_writes();
                editor.set_error(|| format!("Async job failed: {}", e));
                None
            }
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
            let Some(action) = self.after_writes.pop_front() else { break };
            if let Err(error) = action(editor) {
                self.cancel_after_writes();
                editor.set_error(|| error.to_string());
                break;
            }
        }
    }

    /// Blocks until all the jobs that need to be waited on are done.
    #[cfg(test)]
    pub async fn finish(
        &mut self,
        editor: &mut Editor,
        mut compositor: Option<&mut Compositor>,
    ) -> anyhow::Result<()> {
        log::debug!("waiting on jobs...");
        let mut wait_futures = std::mem::take(&mut self.wait_futures);
        let mut first_error = None;

        while let Some(job) = wait_futures.next().await {
            match job {
                Ok(callback) => {
                    if let Some(callback) = callback {
                        // clippy doesn't realize this is an error without the derefs
                        #[allow(clippy::needless_option_as_deref)]
                        if let Some(job) = match callback {
                            Callback::EditorCompositor(call) if compositor.is_some() => {
                                call(editor, compositor.as_deref_mut().unwrap());
                                None
                            }
                            Callback::Editor(call) => {
                                call(editor);
                                None
                            }
                            Callback::Followup(call) => call(editor)?,

                            // skip callbacks for which we don't have the necessary references
                            _ => None,
                        } && job.wait
                        {
                            wait_futures.push(job.future);
                        }
                    }
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
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

#[cfg(all(test, feature = "integration"))]
mod completion_tests {
    use std::sync::Arc;

    use super::*;
    use crate::{application::Application, args::Args, config::Config};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_required_jobs_do_not_skip_callbacks_or_their_followups() -> anyhow::Result<()> {
        let mut config = Config::default();
        config.editor.lsp.enable = false;
        config.editor.files.watch.enable = false;
        config.editor.files.reload.poll.enable = false;
        let mut app = Application::new(
            Args::default(),
            config,
            language::syntax::Loader::new(
                toml::from_str("language = []")?,
                loader::syntax::Resources::default(),
            )?,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;
        let mut jobs = Jobs::new();
        let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let captured = applied.clone();
        jobs.add(Job::new(async { anyhow::bail!("first failure") }).wait_before_exiting());
        let continuation = Job::with_callback(async move {
            Ok(Callback::Editor(Box::new(move |_| {
                captured.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })))
        })
        .wait_before_exiting();
        let followup = Callback::Followup(Box::new(move |_| Ok(Some(continuation))));
        jobs.add(Job::with_callback(async { Ok(followup) }).wait_before_exiting());
        let error = jobs.finish(&mut app.editor, None).await.unwrap_err();
        assert_eq!(error.to_string(), "first failure");
        assert_eq!(applied.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(jobs.wait_futures.is_empty());
        assert!(app.close().await.is_empty());
        Ok(())
    }
}
