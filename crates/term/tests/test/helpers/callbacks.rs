//! Controllable callback queues for checking publication after editor state changes.
use tokio::sync::mpsc;
use view::callbacks::{EditorCallback, EditorCallbackSender};

pub fn channel() -> (
    EditorCallbackSender,
    mpsc::UnboundedReceiver<EditorCallback>,
) {
    unbounded(|_, callback| callback)
}

/// Keep the blocking/async distinction when a test checks how work is delivered.
pub fn unbounded<T: Send + 'static>(
    wrap: impl Fn(bool, EditorCallback) -> T + Clone + Send + Sync + 'static,
) -> (EditorCallbackSender, mpsc::UnboundedReceiver<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let blocking = tx.clone();
    let wrap_blocking = wrap.clone();
    let sender = EditorCallbackSender::new(
        move |callback| {
            let _ = tx.send(wrap(false, callback));
            async {}
        },
        move |callback| {
            let _ = blocking.send(wrap_blocking(true, callback));
        },
    );
    (sender, rx)
}

/// Several handlers can publish tagged callbacks into one bounded queue.
pub fn bounded_sender<T: Send + 'static>(
    tx: &mpsc::Sender<T>,
    wrap: impl Fn(bool, EditorCallback) -> T + Clone + Send + Sync + 'static,
) -> EditorCallbackSender {
    let tx = tx.clone();
    let blocking = tx.clone();
    let wrap_blocking = wrap.clone();
    EditorCallbackSender::new(
        move |callback| {
            let tx = tx.clone();
            let callback = wrap(false, callback);
            async move {
                let _ = tx.send(callback).await;
            }
        },
        move |callback| event::send_blocking(&blocking, wrap_blocking(true, callback)),
    )
}
