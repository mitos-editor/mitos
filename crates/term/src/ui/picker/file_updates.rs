//! Debounced filesystem refreshes that keep the current results visible while loading.

use std::{path::Path, sync::Arc, time::Duration};

use event::{TaskController, TaskHandle};
use futures_util::future::BoxFuture;
use tokio::{
    sync::{broadcast, oneshot},
    time::Instant,
};
use view::{
    file_watcher::{EventType, Events},
    DocumentId, Editor,
};

use super::{preview::is_ignore_file, Picker};

type Refresh<T, D> =
    fn(&str, &Editor, Arc<D>, TaskHandle) -> BoxFuture<'static, anyhow::Result<Vec<T>>>;

pub(super) struct FileUpdates<T, D> {
    events: broadcast::Receiver<Events>,
    root: fn(&D) -> &Path,
    refresh: Refresh<T, D>,
    same: fn(&T, &T) -> bool,
    checked: Instant,
    dirty: bool,
    task: TaskController,
    pending: Option<Pending<T, D>>,
    include_modified: bool,
    include_vcs: bool,
    include_documents: bool,
    documents: Vec<(DocumentId, i32, std::path::PathBuf)>,
}

fn document_versions(editor: &Editor, root: &Path) -> Vec<(DocumentId, i32, std::path::PathBuf)> {
    let mut versions: Vec<_> = editor
        .documents()
        .filter_map(|doc| {
            let path = stdx::path::canonicalize_existing(doc.path()?);
            path.starts_with(root)
                .then(|| (doc.id(), doc.version(), path))
        })
        .collect();
    versions.sort_unstable_by_key(|(id, _, _)| *id);
    versions
}

struct Pending<T, D> {
    version: usize,
    data: Arc<D>,
    result: oneshot::Receiver<anyhow::Result<Vec<T>>>,
}

impl<T, D> FileUpdates<T, D> {
    pub(super) fn stop(&mut self) {
        self.task.cancel();
        self.pending = None;
        self.dirty = true;
    }
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Picker<T, D> {
    pub(crate) fn with_file_updates(
        mut self,
        editor: &Editor,
        root: fn(&D) -> &Path,
        refresh: Refresh<T, D>,
    ) -> Self
    where
        T: PartialEq,
    {
        self.results.selection_resolver = Some(Box::new(|old, new, cursor| {
            let selected = old.get_matched_item(cursor)?;
            new.matched_items(0..new.matched_item_count())
                .position(|item| item.data == selected.data)
                .map(|index| index as u32)
        }));
        self.file_updates = Some(FileUpdates {
            events: editor.file_watcher.subscribe(),
            root,
            refresh,
            same: T::eq,
            checked: Instant::now(),
            dirty: false,
            task: TaskController::new(),
            pending: None,
            include_modified: false,
            include_vcs: false,
            include_documents: false,
            documents: document_versions(
                editor,
                &stdx::path::canonicalize_existing(root(&self.results.editor_data)),
            ),
        });
        self
    }

    /// Search and VCS rows depend on file contents as well as directory membership.
    pub(crate) fn refresh_on_content_changes(mut self) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.include_modified = true;
        }
        self
    }

    /// Text search also depends on unsaved editor buffers, independently of disk events.
    pub(crate) fn refresh_on_document_changes(mut self) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.include_documents = true;
        }
        self
    }

    /// Linked-worktree metadata can live outside the picker's worktree root.
    pub(crate) fn refresh_on_vcs_changes(mut self) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.include_vcs = true;
        }
        self
    }

    pub(super) fn refresh_files(&mut self, editor: &Editor) {
        use broadcast::error::TryRecvError;
        use oneshot::error::TryRecvError as ResultError;

        let Some(mut updates) = self.file_updates.take() else {
            return;
        };
        let root = stdx::path::canonicalize_existing((updates.root)(&self.results.editor_data));
        if updates.include_documents {
            let documents = document_versions(editor, &root);
            if documents != updates.documents {
                updates.dirty = true;
                updates.documents = documents;
            }
        }
        loop {
            match updates.events.try_recv() {
                Ok(events) => {
                    updates.dirty |= events.iter().any(|event| {
                        let path = event.path.as_std_path();
                        event.ty != EventType::Tempfile
                            && (is_ignore_file(path)
                                || (updates.include_vcs && editor.file_watcher.is_vcs_path(path))
                                || ((updates.include_modified
                                    || event.ty != EventType::Modified
                                    || path.is_dir())
                                    && (path.starts_with(&root) || root.starts_with(path))))
                    })
                }
                Err(TryRecvError::Lagged(_)) => updates.dirty = true,
                Err(TryRecvError::Closed) => {
                    updates.stop();
                    updates.events = editor.file_watcher.subscribe();
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        if updates.pending.as_ref().is_some_and(|pending| {
            !Arc::ptr_eq(&pending.data, &self.results.editor_data)
                || pending.version
                    != self
                        .results
                        .version
                        .load(std::sync::atomic::Ordering::Relaxed)
        }) {
            updates.stop();
        }
        if let Some(pending) = &mut updates.pending {
            match pending.result.try_recv() {
                Ok(result) => {
                    updates.pending = None;
                    updates.checked = Instant::now();
                    // Publish this valid scan even when more changes arrived.
                    // Keep dirty set so another scan follows without starving results.
                    match result {
                        Ok(options) => {
                            self.results.matcher.tick(0);
                            let snapshot = self.results.matcher.snapshot();
                            let same = snapshot.item_count() as usize == options.len()
                                && options.iter().enumerate().all(|(index, item)| {
                                    snapshot
                                        .get_item(index as u32)
                                        .is_some_and(|old| (updates.same)(old.data, item))
                                });
                            if !same {
                                self.refresh_options(options, self.results.editor_data.clone());
                            }
                        }
                        Err(error) => editor.status_sender().report_blocking(error),
                    }
                }
                Err(ResultError::Closed) => updates.pending = None,
                Err(ResultError::Empty) => {}
            }
        }

        let poll = &editor.config().files.reload.poll;
        if updates.pending.is_none()
            && poll.enable
            && updates.checked.elapsed() >= Duration::from_millis(poll.interval.max(100))
        {
            updates.dirty = true;
        }
        if updates.dirty
            && updates.pending.is_none()
            && self.results.matcher.active_injectors() == 0
            && !self.results.is_refreshing()
        {
            updates.dirty = false;
            updates.checked = Instant::now();
            let cancel = updates.task.restart();
            let version = self
                .results
                .version
                .load(std::sync::atomic::Ordering::Relaxed);
            let load = (updates.refresh)(
                &self.primary_query(),
                editor,
                self.results.editor_data.clone(),
                cancel.clone(),
            );
            let (tx, result) = oneshot::channel();
            updates.pending = Some(Pending {
                version,
                data: self.results.editor_data.clone(),
                result,
            });
            let redraw = self.redraw.clone();
            tokio::spawn(async move {
                let work = async {
                    let result = load.await;
                    let _ = tx.send(result);
                    redraw.request();
                };
                event::cancelable_future(work, cancel).await;
            });
        }
        self.file_updates = Some(updates);
    }
}

#[cfg(all(test, feature = "integration"))]
#[path = "file_updates_tests.rs"]
mod tests;
