//! Ordered writes for each document, polled concurrently across documents.

use std::collections::HashMap;
use std::future::Future;

use futures_util::{
    stream::{self, Flatten, Once, SelectAll},
    StreamExt,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    document::{DocumentSavedEventFuture, DocumentSavedEventResult},
    DocumentId,
};

type DocumentWrites = Flatten<UnboundedReceiverStream<Once<DocumentSavedEventFuture>>>;

/// Owns write admission, per-document ordering, and pending-completion accounting.
/// Closing a document rejects new writes while retaining every accepted write.
#[derive(Default)]
pub(crate) struct WriteQueue {
    senders: HashMap<DocumentId, UnboundedSender<Once<DocumentSavedEventFuture>>>,
    completions: SelectAll<DocumentWrites>,
    pending: usize,
}

impl WriteQueue {
    pub(crate) fn open_document(&mut self, document: DocumentId) {
        let (sender, receiver) = unbounded_channel();
        assert!(
            self.senders.insert(document, sender).is_none(),
            "document already has a write queue"
        );
        // Flatten polls one write to completion before receiving the next one
        // for this document. SelectAll keeps independent documents progressing.
        self.completions
            .push(UnboundedReceiverStream::new(receiver).flatten());
    }

    pub(crate) fn close_document(&mut self, document: DocumentId) {
        self.senders.remove(&document);
    }

    pub(crate) fn enqueue(
        &mut self,
        document: DocumentId,
        write: impl Future<Output = DocumentSavedEventResult> + Send + 'static,
    ) -> anyhow::Result<()> {
        self.senders
            .get(&document)
            .ok_or_else(|| anyhow::format_err!("saves are closed for this document!"))?
            .send(stream::once(Box::pin(write)))
            .map_err(|error| anyhow::anyhow!("failed to send save event: {error}"))?;
        self.pending += 1;
        Ok(())
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending != 0
    }

    /// Both event delivery and shutdown draining consume completions here, so
    /// successful and failed writes are counted exactly once on either path.
    pub(crate) async fn next(&mut self) -> Option<DocumentSavedEventResult> {
        let completion = self.completions.next().await?;
        self.pending -= 1;
        Some(completion)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, SystemTime},
    };

    use crate::document::DocumentSavedEvent;

    use super::*;

    fn completed(document: DocumentId, revision: usize) -> DocumentSavedEventResult {
        Ok(DocumentSavedEvent {
            revision,
            save_time: SystemTime::now(),
            doc_id: document,
            path: "document.txt".into(),
            text: editor_core::Rope::new(),
        })
    }

    #[tokio::test]
    async fn writes_preserve_document_order_without_blocking_other_documents() -> anyhow::Result<()>
    {
        let mut queue = WriteQueue::default();
        let first = DocumentId::new(1);
        let second = DocumentId::new(2);
        queue.open_document(first);
        queue.open_document(second);
        let (release, gate) = tokio::sync::oneshot::channel();
        queue.enqueue(first, async move {
            gate.await?;
            completed(first, 1)
        })?;
        let second_write_started = Arc::new(AtomicBool::new(false));
        let started = second_write_started.clone();
        queue.enqueue(first, async move {
            started.store(true, Ordering::Relaxed);
            completed(first, 2)
        })?;
        queue.enqueue(second, async move { completed(second, 1) })?;
        // Closing admission must not discard either accepted write.
        queue.close_document(first);
        let other = tokio::time::timeout(Duration::from_secs(5), queue.next())
            .await?
            .unwrap()?;
        assert_eq!(other.doc_id, second);
        assert!(!second_write_started.load(Ordering::Relaxed));
        // The event loop may drop a pending next() when another event wins.
        // Its in-progress write and completion accounting must remain owned here.
        assert!(futures_util::poll!(std::pin::pin!(queue.next())).is_pending());
        assert!(queue.has_pending());
        release.send(()).unwrap();
        for revision in [1, 2] {
            let event = queue.next().await.unwrap()?;
            assert_eq!(event.doc_id, first);
            assert_eq!(event.revision, revision);
        }
        assert!(!queue.has_pending());
        Ok(())
    }

    #[tokio::test]
    async fn failures_complete_once_and_closed_documents_reject_new_writes() -> anyhow::Result<()> {
        let mut queue = WriteQueue::default();
        let document = DocumentId::new(1);
        queue.open_document(document);
        queue.enqueue(document, async { anyhow::bail!("write failed") })?;
        queue.enqueue(document, async move { completed(document, 2) })?;
        queue.close_document(document);
        let error = queue
            .enqueue(document, async move { completed(document, 3) })
            .unwrap_err();
        assert_eq!(error.to_string(), "saves are closed for this document!");
        assert_eq!(
            queue.next().await.unwrap().unwrap_err().to_string(),
            "write failed"
        );
        assert!(queue.has_pending());
        assert_eq!(queue.next().await.unwrap()?.revision, 2);
        assert!(!queue.has_pending());
        assert!(queue.next().await.is_none());
        Ok(())
    }
}
