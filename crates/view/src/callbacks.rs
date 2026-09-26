//! Explicit delivery of background results to their owning editor.

use std::{future::Future, sync::Arc};

use futures_util::future::BoxFuture;

use crate::Editor;

pub type EditorCallback = Box<dyn FnOnce(&mut Editor) + Send>;

/// A completion destination bound to one editor by its application.
///
/// The frontend queues callbacks and runs them on that editor's mutation path.
/// Sending waits for queue capacity; a closed destination may discard the callback.
#[derive(Clone)]
pub struct EditorCallbackSender {
    send: Arc<dyn Fn(EditorCallback) -> BoxFuture<'static, ()> + Send + Sync>,
}

impl EditorCallbackSender {
    pub fn new<F, Fut>(send: F) -> Self
    where
        F: Fn(EditorCallback) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self {
            send: Arc::new(move |callback| Box::pin(send(callback))),
        }
    }

    pub async fn send(&self, callback: impl FnOnce(&mut Editor) + Send + 'static) {
        (self.send)(Box::new(callback)).await;
    }
}
