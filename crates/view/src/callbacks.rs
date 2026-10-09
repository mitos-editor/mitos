//! Explicit delivery of background results to their owning editor.

use std::{
    fmt,
    future::Future,
    sync::{Arc, Mutex},
};

use futures_util::future::BoxFuture;

use crate::Editor;

pub type EditorCallback = Box<dyn FnOnce(&mut Editor) + Send>;
type TrySendEditorCallback = dyn Fn(EditorCallback) -> Result<(), EditorCallback> + Send + Sync;

/// Completion of one accepted native task belonging to a command invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskOutcome {
    Success,
    Error(String),
    Cancelled(String),
}

/// A frontend-owned invocation can observe native work without coupling the
/// editor model to terminal command dispatch or callback implementation details.
pub trait InvocationTasks: Send + Sync {
    fn started(&self);
    fn finished(&self, outcome: TaskOutcome);

    /// The host accepted work whose eventual completion is not observable by
    /// this invocation. Frontends must report dispatch rather than success.
    fn detached(&self) {}
}

#[derive(Clone)]
pub(crate) struct TaskCompletion(Arc<Mutex<Option<Arc<dyn InvocationTasks>>>>);

impl TaskCompletion {
    pub(crate) fn finish(&self, outcome: TaskOutcome) {
        let observer = self.0.lock().unwrap().take();
        if let Some(observer) = observer {
            observer.finished(outcome);
        }
    }
}

pub(crate) struct InvocationTask(TaskCompletion, &'static str);

impl InvocationTask {
    pub(crate) fn new(observer: Option<Arc<dyn InvocationTasks>>) -> Option<Self> {
        Self::new_named(observer, "write cancelled before completion")
    }

    pub(crate) fn new_named(
        observer: Option<Arc<dyn InvocationTasks>>,
        cancellation: &'static str,
    ) -> Option<Self> {
        observer.map(|observer| {
            observer.started();
            Self(
                TaskCompletion(Arc::new(Mutex::new(Some(observer)))),
                cancellation,
            )
        })
    }

    pub(crate) fn completion(&self) -> TaskCompletion {
        self.0.clone()
    }

    pub(crate) fn finish(self, outcome: TaskOutcome) {
        self.0.finish(outcome);
    }
}

impl Drop for InvocationTask {
    fn drop(&mut self) {
        self.0.finish(TaskOutcome::Cancelled(self.1.into()));
    }
}

/// A completion destination bound to one editor by its application.
///
/// The frontend queues callbacks and runs them on that editor's mutation path.
/// Async sends wait for queue capacity. Synchronous sends use the frontend's
/// bounded-wait policy; full or closed destinations may discard those callbacks.
/// Producers that retain their own work should use `try_send` and an explicit
/// frontend polling path to recover capacity without blocking the editor.
#[derive(Clone)]
pub struct EditorCallbackSender {
    send: Arc<dyn Fn(EditorCallback) -> BoxFuture<'static, ()> + Send + Sync>,
    send_blocking: Arc<dyn Fn(EditorCallback) + Send + Sync>,
    try_send: Option<Arc<TrySendEditorCallback>>,
}

impl fmt::Debug for EditorCallbackSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditorCallbackSender")
            .finish_non_exhaustive()
    }
}

impl EditorCallbackSender {
    pub fn new<F, Fut>(
        send: F,
        send_blocking: impl Fn(EditorCallback) + Send + Sync + 'static,
    ) -> Self
    where
        F: Fn(EditorCallback) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self {
            send: Arc::new(move |callback| Box::pin(send(callback))),
            send_blocking: Arc::new(send_blocking),
            try_send: None,
        }
    }

    /// Adds lossless nonblocking delivery. A full or closed destination returns
    /// ownership of the callback so its producer can retain work and poll later.
    pub fn with_try_send(
        mut self,
        send: impl Fn(EditorCallback) -> Result<(), EditorCallback> + Send + Sync + 'static,
    ) -> Self {
        self.try_send = Some(Arc::new(send));
        self
    }

    pub fn try_send(
        &self,
        callback: impl FnOnce(&mut Editor) + Send + 'static,
    ) -> Result<(), EditorCallback> {
        let callback = Box::new(callback);
        match &self.try_send {
            Some(send) => send(callback),
            None => Err(callback),
        }
    }

    pub async fn send(&self, callback: impl FnOnce(&mut Editor) + Send + 'static) {
        (self.send)(Box::new(callback)).await;
    }

    pub fn send_blocking(&self, callback: impl FnOnce(&mut Editor) + Send + 'static) {
        (self.send_blocking)(Box::new(callback));
    }
}
