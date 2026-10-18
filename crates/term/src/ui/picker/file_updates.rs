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

type Refresh<T, D> = fn(&Editor, Arc<D>, TaskHandle) -> BoxFuture<'static, anyhow::Result<Vec<T>>>;

pub(super) struct FileUpdates<T, D> {
    events: broadcast::Receiver<Events>,
    root: fn(&D) -> &Path,
    refresh: Refresh<T, D>,
    item_path: fn(&T) -> &Path,
    same: fn(&T, &T) -> bool,
    checked: Instant,
    dirty: bool,
    task: TaskController,
    pending: Option<Pending<T, D>>,
}

struct Pending<T, D> {
    data: Arc<D>,
    result: oneshot::Receiver<anyhow::Result<Vec<T>>>,
}

#[cfg(all(test, feature = "integration"))]
#[path = "file_updates_tests.rs"]
mod tests;

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
        T: PartialEq,
    {
        self.file_updates = Some(FileUpdates {
            events: editor.file_watcher.subscribe(),
            root,
            refresh,
            item_path,
            same: T::eq,
            checked: Instant::now(),
            dirty: false,
            task: TaskController::new(),
            pending: None,
        });
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
                                || (event.ty != EventType::Modified
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

        if updates
            .pending
            .as_ref()
            .is_some_and(|pending| !Arc::ptr_eq(&pending.data, &self.editor_data))
        {
            updates.stop();
        }
        if let Some(pending) = &mut updates.pending {
            match pending.result.try_recv() {
                Ok(result) => {
                    updates.pending = None;
                    updates.checked = Instant::now();
                    // A new batch arrived during the scan. Keep the old rows until its replacement finishes.
                    if !updates.dirty {
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
                                    self.restore_selection = self
                                        .selection()
                                        .map(|item| (updates.item_path)(item).to_owned());
                                    self.refreshing = true;
                                    self.reset_options_shared(options, self.editor_data.clone());
                                }
                            }
                            Err(error) => editor.status_sender().report_blocking(error),
                        }
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
            let load = (updates.refresh)(editor, self.editor_data.clone(), cancel.clone());
            let (tx, result) = oneshot::channel();
            updates.pending = Some(Pending {
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
        let Some(path) = self.restore_selection.take() else {
            return;
        };
        let Some(updates) = &self.file_updates else {
            return;
        };
        let snapshot = self.matcher.snapshot();
        if let Some(index) = snapshot
            .matched_items(0..snapshot.matched_item_count())
            .position(|item| (updates.item_path)(item.data) == path)
        {
            self.cursor = index as u32;
        }
    }
}
