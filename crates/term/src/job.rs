use event::send_blocking;
use event::status::StatusMessage;
use view::callbacks::{EditorCallback, EditorCallbackSender};
use view::Editor;

use crate::compositor::Compositor;

use futures_util::future::{BoxFuture, Future, FutureExt};
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub type EditorCompositorCallback = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;
pub type EditorCallbackFollowup = Box<dyn FnOnce(&mut Editor) -> Option<Job> + Send>;

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

pub enum Callback {
    EditorCompositor(EditorCompositorCallback),
    Editor(EditorCallback),
    Followup(EditorCallbackFollowup),
}

pub type JobFuture = BoxFuture<'static, anyhow::Result<Option<Callback>>>;

pub struct Job {
    pub future: BoxFuture<'static, anyhow::Result<Option<Callback>>>,
    /// Do we need to wait for this job to finish before exiting?
    pub wait: bool,
}

pub struct Jobs {
    sender: JobSender,
    /// jobs that need to complete before we exit.
    pub wait_futures: FuturesUnordered<JobFuture>,
    pub callbacks: Receiver<Callback>,
    pub status_messages: Receiver<StatusMessage>,
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
        let status_messages = event::status::setup();
        Self {
            sender,
            wait_futures: FuturesUnordered::new(),
            callbacks: rx,
            status_messages,
        }
    }

    pub fn sender(&self) -> JobSender {
        self.sender.clone()
    }

    /// Bind editor-only completions to this application's queue.
    pub fn editor_callback_sender(&self) -> EditorCallbackSender {
        let sender = self.sender.0.clone();
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
        &self,
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
            },
            Err(e) => {
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

    /// Blocks until all the jobs that need to be waited on are done.
    pub async fn finish(
        &mut self,
        editor: &mut Editor,
        mut compositor: Option<&mut Compositor>,
    ) -> anyhow::Result<()> {
        log::debug!("waiting on jobs...");
        let mut wait_futures = std::mem::take(&mut self.wait_futures);

        while let (Some(job), tail) = StreamExt::into_future(wait_futures).await {
            match job {
                Ok(callback) => {
                    wait_futures = tail;

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
                            Callback::Followup(call) => call(editor),

                            // skip callbacks for which we don't have the necessary references
                            _ => None,
                        } && job.wait
                        {
                            wait_futures.push(job.future);
                        }
                    }
                }
                Err(e) => {
                    self.wait_futures = tail;
                    return Err(e);
                }
            }
        }

        Ok(())
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
