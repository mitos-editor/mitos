use arc_swap::ArcSwapOption;
use event::status::StatusMessage;
use event::{runtime_local, send_blocking};
use std::sync::Arc;
use view::Editor;

use crate::compositor::Compositor;

use futures_util::future::{BoxFuture, Future, FutureExt};
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub type EditorCompositorCallback = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;
pub type EditorCallback = Box<dyn FnOnce(&mut Editor) + Send>;
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
}

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
        }
    }

    /// Use this editor's queue for callbacks dispatched by event hooks.
    /// Temporary job collections, such as those used by auto-save, do not replace it.
    pub(crate) fn set_current(&self) {
        JOB_QUEUE.store(Some(self.sender.clone()));
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
    async fn callbacks_follow_the_selected_queue_and_jobs_keep_their_owner() {
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
