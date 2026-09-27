//! Explicit delivery of background results to their owning editor.

use std::{fmt, future::Future, sync::Arc};

use futures_util::future::BoxFuture;

use crate::Editor;

pub type EditorCallback = Box<dyn FnOnce(&mut Editor) + Send>;

/// A completion destination bound to one editor by its application.
///
/// The frontend queues callbacks and runs them on that editor's mutation path.
/// Async sends wait for queue capacity. Synchronous sends use the frontend's
/// bounded-wait policy; full or closed destinations may discard those callbacks.
#[derive(Clone)]
pub struct EditorCallbackSender {
    send: Arc<dyn Fn(EditorCallback) -> BoxFuture<'static, ()> + Send + Sync>,
    send_blocking: Arc<dyn Fn(EditorCallback) + Send + Sync>,
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
        }
    }

    pub async fn send(&self, callback: impl FnOnce(&mut Editor) + Send + 'static) {
        (self.send)(Box::new(callback)).await;
    }

    pub fn send_blocking(&self, callback: impl FnOnce(&mut Editor) + Send + 'static) {
        (self.send_blocking)(Box::new(callback));
    }
}
