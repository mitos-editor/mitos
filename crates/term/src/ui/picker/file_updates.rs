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
    Editor,
};

use super::{preview::is_ignore_file, Picker};

type Refresh<T, D> =
    fn(&str, &Editor, Arc<D>, TaskHandle) -> BoxFuture<'static, anyhow::Result<Vec<T>>>;

pub(super) struct FileUpdates<T, D> {
    events: broadcast::Receiver<Events>,
    root: fn(&D) -> &Path,
    refresh: Refresh<T, D>,
    item_path: fn(&T) -> &Path,
    same: fn(&T, &T) -> bool,
    clone_item: fn(&T) -> T,
    checked: Instant,
    dirty: bool,
    task: TaskController,
    pending: Option<Pending<T, D>>,
    include_modified: bool,
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
        item_path: fn(&T) -> &Path,
    ) -> Self
    where
        T: PartialEq + Clone,
    {
        self.file_updates = Some(FileUpdates {
            events: editor.file_watcher.subscribe(),
            root,
            refresh,
            item_path,
            same: T::eq,
            clone_item: T::clone,
            checked: Instant::now(),
            dirty: false,
            task: TaskController::new(),
            pending: None,
            include_modified: false,
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

    pub(super) fn refresh_files(&mut self, editor: &Editor) {
        use broadcast::error::TryRecvError;
        use oneshot::error::TryRecvError as ResultError;

        let Some(mut updates) = self.file_updates.take() else {
            return;
        };
        let root = stdx::path::canonicalize_existing((updates.root)(&self.editor_data));
        loop {
            match updates.events.try_recv() {
                Ok(events) => {
                    updates.dirty |= events.iter().any(|event| {
                        let path = event.path.as_std_path();
                        event.ty != EventType::Tempfile
                            && (is_ignore_file(path)
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
            !Arc::ptr_eq(&pending.data, &self.editor_data)
                || pending.version != self.version.load(std::sync::atomic::Ordering::Relaxed)
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
                            self.matcher.tick(0);
                            let snapshot = self.matcher.snapshot();
                            let same = snapshot.item_count() as usize == options.len()
                                && options.iter().enumerate().all(|(index, item)| {
                                    snapshot
                                        .get_item(index as u32)
                                        .is_some_and(|old| (updates.same)(old.data, item))
                                });
                            if !same {
                                self.restore_selection = self.selection().map(updates.clone_item);
                                self.refreshing = true;
                                self.reset_options_shared(options, self.editor_data.clone());
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
            && self.matcher.active_injectors() == 0
            && !self.refreshing
        {
            updates.dirty = false;
            updates.checked = Instant::now();
            let cancel = updates.task.restart();
            let version = self.version.load(std::sync::atomic::Ordering::Relaxed);
            let load = (updates.refresh)(
                &self.primary_query(),
                editor,
                self.editor_data.clone(),
                cancel.clone(),
            );
            let (tx, result) = oneshot::channel();
            updates.pending = Some(Pending {
                version,
                data: self.editor_data.clone(),
                result,
            });
            let redraw = self.redraw.clone();
            tokio::spawn(async move {
                let work = async {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let result = load.await;
                    let _ = tx.send(result);
                    redraw.request();
                };
                event::cancelable_future(work, cancel).await;
            });
        }
        self.file_updates = Some(updates);
    }

    pub(super) fn restore_file_selection(&mut self) {
        let Some(selected) = self.restore_selection.take() else {
            return;
        };
        let Some(updates) = &self.file_updates else {
            return;
        };
        let snapshot = self.matcher.snapshot();
        let mut items = snapshot.matched_items(0..snapshot.matched_item_count());
        let index = items
            .position(|item| (updates.same)(item.data, &selected))
            .or_else(|| {
                snapshot
                    .matched_items(0..snapshot.matched_item_count())
                    .position(|item| {
                        (updates.item_path)(item.data) == (updates.item_path)(&selected)
                    })
            });
        if let Some(index) = index {
            self.cursor = index as u32;
        }
    }
}

#[cfg(all(test, feature = "integration"))]
#[path = "file_updates_tests.rs"]
mod tests;
