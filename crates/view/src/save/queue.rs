//! Ordered writes for each document, polled concurrently across documents.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::future::Future;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

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

#[derive(Debug, Default)]
struct PathWrites {
    count: usize,
    exclusive: usize,
}

/// Successful events retain their paths until the editor applies or drops them.
/// Cloned events share an idempotent release, including delayed frontend delivery.
#[derive(Debug)]
pub(crate) struct WriteReservation {
    paths: Mutex<Option<Vec<PathBuf>>>,
    active: Arc<Mutex<HashMap<PathBuf, PathWrites>>>,
    exclusive: bool,
}

impl WriteReservation {
    pub(crate) fn release(&self) {
        if let Some(paths) = self.paths.lock().take() {
            let mut active = self.active.lock();
            for path in paths {
                let count = active.get_mut(&path).expect("queued path is reserved");
                count.count -= 1;
                count.exclusive -= usize::from(self.exclusive);
                if count.count == 0 {
                    active.remove(&path);
                }
            }
        }
    }
}

impl Drop for WriteReservation {
    fn drop(&mut self) {
        self.release();
    }
}

/// Owns write admission, per-document ordering, and pending-completion accounting.
/// Closing a document rejects new writes while retaining every accepted write.
#[derive(Default)]
pub(crate) struct WriteQueue {
    senders: HashMap<DocumentId, UnboundedSender<Once<DocumentSavedEventFuture>>>,
    completions: SelectAll<DocumentWrites>,
    pending: usize,
    paths: Arc<Mutex<HashMap<PathBuf, PathWrites>>>,
    blocked: Vec<Weak<Vec<PathBuf>>>,
}

/// Suspends write admission for an external filesystem operation. Dropping it releases the paths.
#[must_use = "Keep the write block alive until the filesystem operation completes"]
pub struct FileWriteBlock {
    _paths: Arc<Vec<PathBuf>>,
}

impl FileWriteBlock {
    pub fn contains(&self, path: &Path) -> bool {
        blocked_by(path, &self._paths)
    }
}

fn blocked_by(path: &Path, blocked: &[PathBuf]) -> bool {
    let path = stdx::path::canonicalize(path);
    let matches = |path: &Path| blocked.iter().any(|blocked| path.starts_with(blocked));
    if matches(&path) {
        return true;
    }
    // Preserve the leaf: an external operation may replace a file with a symlink.
    if let Some((parent, name)) = path.parent().zip(path.file_name())
        && matches(&stdx::path::canonicalize_existing(parent).join(name))
    {
        return true;
    }
    // Resolve ancestor aliases independently of a newly introduced symlink farther down.
    path.ancestors()
        .any(|ancestor| matches(&stdx::path::canonicalize_existing(ancestor)))
}

impl WriteQueue {
    pub(crate) fn block(&mut self, paths: Vec<PathBuf>) -> anyhow::Result<FileWriteBlock> {
        anyhow::ensure!(
            !self.has_pending(),
            "Wait for pending saves before changing files on disk"
        );
        self.blocked.retain(|paths| paths.strong_count() != 0);
        let mut paths: Vec<_> = paths
            .into_iter()
            .flat_map(|path| {
                [
                    stdx::path::canonicalize(&path),
                    stdx::path::canonicalize_existing(&path),
                ]
            })
            .collect();
        paths.sort();
        paths.dedup();
        let paths = Arc::new(paths);
        self.blocked.push(Arc::downgrade(&paths));
        Ok(FileWriteBlock { _paths: paths })
    }

    pub(crate) fn is_blocked(&self, path: &Path) -> bool {
        if self.blocked.iter().all(|paths| paths.strong_count() == 0) {
            return false;
        }
        self.blocked
            .iter()
            .filter_map(Weak::upgrade)
            .any(|paths| blocked_by(path, &paths))
    }
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
        mut paths: Vec<PathBuf>,
        write: impl Future<Output = DocumentSavedEventResult> + Send + 'static,
    ) -> anyhow::Result<()> {
        paths.sort_unstable();
        paths.dedup();
        let sender = self
            .senders
            .get(&document)
            .ok_or_else(|| anyhow::format_err!("saves are closed for this document!"))?;
        anyhow::ensure!(
            !self
                .paths
                .lock()
                .iter()
                .any(|(reserved, writes)| writes.exclusive > 0
                    && paths
                        .iter()
                        .any(|path| reserved.starts_with(path) || path.starts_with(reserved))),
            "A replacement write is pending for this path"
        );
        let reservation = Self::reserve(&self.paths, paths, false);
        sender
            .send(stream::once(Box::pin(async move {
                let mut result = write.await;
                if let Ok(event) = &mut result {
                    event.reservation = Some(reservation);
                }
                result
            })))
            .map_err(|error| anyhow::anyhow!("failed to send save event: {error}"))?;
        self.pending += 1;
        Ok(())
    }

    pub(crate) fn reserve_paths(&self, mut paths: Vec<PathBuf>) -> Arc<WriteReservation> {
        paths.sort_unstable();
        paths.dedup();
        Self::reserve(&self.paths, paths, true)
    }

    fn reserve(
        active: &Arc<Mutex<HashMap<PathBuf, PathWrites>>>,
        paths: Vec<PathBuf>,
        exclusive: bool,
    ) -> Arc<WriteReservation> {
        {
            let mut active = active.lock();
            for path in &paths {
                let writes = active.entry(path.clone()).or_default();
                writes.count += 1;
                writes.exclusive += usize::from(exclusive);
            }
        }
        Arc::new(WriteReservation {
            paths: Mutex::new(Some(paths)),
            active: active.clone(),
            exclusive,
        })
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending != 0
    }

    /// A directory operation also conflicts with writes to any of its children.
    pub(crate) fn conflicts_with(&self, path: &Path) -> bool {
        self.paths
            .lock()
            .keys()
            .any(|reserved| reserved.starts_with(path) || path.starts_with(reserved))
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

    #[cfg(unix)]
    #[test]
    fn write_blocks_cover_aliases_and_files_replaced_by_symlinks() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let file = root.path().join("file");
        std::fs::write(&file, "original")?;
        let target = outside.path().join("target");
        std::fs::write(&target, "outside")?;
        let alias = outside.path().join("alias");
        symlink(root.path(), &alias)?;
        let mut queue = WriteQueue::default();
        let block = queue.block(vec![file.clone()])?;
        std::fs::remove_file(&file)?;
        symlink(&target, &file)?;
        assert!(queue.is_blocked(&file));
        assert!(block.contains(&alias.join("file")));
        assert!(!queue.is_blocked(&target));
        drop(block);
        assert!(!queue.is_blocked(&file));
        let block = queue.block(vec![root.path().into()])?;
        assert!(queue.is_blocked(&alias.join("new-file")));
        assert!(!queue.is_blocked(&outside.path().join("other")));
        drop(block);
        assert!(!queue.is_blocked(&alias.join("new-file")));
        Ok(())
    }

    fn completed(document: DocumentId, revision: usize) -> DocumentSavedEventResult {
        Ok(DocumentSavedEvent {
            revision,
            save_time: SystemTime::now(),
            doc_id: document,
            path: "document.txt".into(),
            text: editor_core::Rope::new(),
            reservation: None,
        })
    }

    #[tokio::test]
    async fn exclusive_file_replacements_reject_other_writes_until_released() {
        let mut queue = WriteQueue::default();
        let document = DocumentId::default();
        let path = PathBuf::from("replacement.txt");
        queue.open_document(document);
        let reservation = queue.reserve_paths(vec![path.clone()]);
        assert!(queue
            .enqueue(document, vec![path.clone()], async {
                Err(anyhow::anyhow!("unused"))
            })
            .is_err());
        drop(reservation);
        assert!(queue
            .enqueue(document, vec![path], async {
                Err(anyhow::anyhow!("unused"))
            })
            .is_ok());
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
        queue.enqueue(first, Vec::new(), async move {
            gate.await?;
            completed(first, 1)
        })?;
        let second_write_started = Arc::new(AtomicBool::new(false));
        let started = second_write_started.clone();
        queue.enqueue(first, Vec::new(), async move {
            started.store(true, Ordering::Relaxed);
            completed(first, 2)
        })?;
        queue.enqueue(second, Vec::new(), async move { completed(second, 1) })?;
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
        queue.enqueue(document, Vec::new(), async {
            anyhow::bail!("write failed")
        })?;
        queue.enqueue(document, Vec::new(), async move { completed(document, 2) })?;
        queue.close_document(document);
        let error = queue
            .enqueue(document, Vec::new(), async move { completed(document, 3) })
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

    #[tokio::test]
    async fn path_reservations_follow_each_completion_and_its_clones() -> anyhow::Result<()> {
        let mut queue = WriteQueue::default();
        let document = DocumentId::new(1);
        queue.open_document(document);
        let path = PathBuf::from("directory/file.txt");
        for revision in [1, 2] {
            queue.enqueue(document, vec![path.clone(), path.clone()], async move {
                completed(document, revision)
            })?;
        }
        let first = queue.next().await.unwrap()?;
        let first_clone = first.clone();
        first.reservation.as_ref().unwrap().release();
        drop(first);
        drop(first_clone);
        assert!(queue.conflicts_with(&path));
        assert!(queue.conflicts_with(Path::new("directory")));
        assert!(!queue.conflicts_with(Path::new("directory-other")));
        let second = queue.next().await.unwrap()?;
        assert!(queue.conflicts_with(&path));
        // Abandoned delivery also releases its reservation.
        drop(second);
        assert!(!queue.conflicts_with(&path));
        Ok(())
    }
}
