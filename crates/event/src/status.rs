//! A queue of async messages/errors that will be shown in the editor

use std::borrow::Cow;
use std::time::Duration;

use tokio::sync::mpsc::{Receiver, Sender};

/// Describes the severity level of a [`StatusMessage`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub enum Severity {
    Hint,
    Info,
    Warning,
    Error,
}

pub struct StatusMessage {
    pub severity: Severity,
    pub message: Cow<'static, str>,
}

impl From<anyhow::Error> for StatusMessage {
    fn from(err: anyhow::Error) -> Self {
        StatusMessage {
            severity: Severity::Error,
            message: err.to_string().into(),
        }
    }
}

impl From<&'static str> for StatusMessage {
    fn from(msg: &'static str) -> Self {
        StatusMessage {
            severity: Severity::Info,
            message: msg.into(),
        }
    }
}

/// A bounded status destination belonging to one editor.
#[derive(Clone, Debug)]
pub struct StatusSender(Sender<StatusMessage>);

impl StatusSender {
    pub fn channel() -> (Self, Receiver<StatusMessage>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(128);
        (Self(sender), receiver)
    }

    /// Discard messages if the owner closed or remains overloaded for 10 ms.
    pub async fn report(&self, message: impl Into<StatusMessage>) {
        let _ = self
            .0
            .send_timeout(message.into(), Duration::from_millis(10))
            .await;
    }

    /// Best-effort delivery from any thread, without needing a Tokio runtime.
    /// Synchronous callers must not wait for the editor to consume a full queue.
    pub fn report_blocking(&self, message: impl Into<StatusMessage>) {
        let _ = self.0.try_send(message.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn senders_keep_their_owner_and_closed_receivers_do_not_block() {
        let (first, mut first_messages) = StatusSender::channel();
        let (second, mut second_messages) = StatusSender::channel();
        first.report("first").await;
        second.report_blocking("second");
        assert_eq!(first_messages.recv().await.unwrap().message, "first");
        assert_eq!(second_messages.recv().await.unwrap().message, "second");
        drop(first_messages);
        first.report("closed").await;
        first.report_blocking("closed");
        second.report("alive").await;
        assert_eq!(second_messages.recv().await.unwrap().message, "alive");
    }

    #[tokio::test]
    async fn an_overloaded_status_queue_does_not_block_another_owner() {
        let (first, _first_messages) = StatusSender::channel();
        let (second, mut second_messages) = StatusSender::channel();
        for _ in 0..128 {
            first.report("queued").await;
        }
        first.report("overloaded").await;
        second.report("alive").await;
        assert_eq!(second_messages.recv().await.unwrap().message, "alive");
    }

    #[test]
    fn synchronous_overload_is_nonblocking_on_background_threads() {
        let (first, mut first_messages) = StatusSender::channel();
        let (second, mut second_messages) = StatusSender::channel();
        for _ in 0..128 {
            first.report_blocking("queued");
        }
        std::thread::spawn(move || {
            first.report_blocking("overloaded");
            second.report_blocking("other owner");
        })
        .join()
        .unwrap();
        assert_eq!(first_messages.len(), 128);
        assert_eq!(first_messages.try_recv().unwrap().message, "queued");
        assert_eq!(second_messages.try_recv().unwrap().message, "other owner");
    }
}
