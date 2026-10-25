//! Debounced filesystem refreshes that keep the current results visible while loading.

use std::{path::Path, sync::Arc};

use event::{TaskController, TaskHandle};
use futures_util::future::BoxFuture;
use tokio::sync::oneshot;
use view::{
    features::workspace_changes::{Documents, Policy, WorkspaceChanges},
    Editor,
};

use super::Picker;

type Refresh<T, D> =
    fn(&str, &Editor, Arc<D>, TaskHandle) -> BoxFuture<'static, anyhow::Result<Vec<T>>>;

pub(super) struct FileUpdates<T, D> {
    changes: WorkspaceChanges,
    root: fn(&D) -> &Path,
    refresh: Refresh<T, D>,
    same: fn(&T, &T) -> bool,
    dirty: bool,
    task: TaskController,
    pending: Option<Pending<T, D>>,
    generation: Option<Generation>,
}

struct Generation {
    current: fn(&Editor) -> u64,
    observed: u64,
}

struct Pending<T, D> {
    version: usize,
    data: Arc<D>,
    result: oneshot::Receiver<anyhow::Result<Vec<T>>>,
}

impl<T, D> FileUpdates<T, D> {
    pub(super) fn is_pending(&self) -> bool {
        self.dirty || self.pending.is_some()
    }

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
            changes: WorkspaceChanges::new(
                editor,
                root(&self.results.editor_data),
                Policy {
                    ignore_files: true,
                    ..Policy::default()
                },
            ),
            root,
            refresh,
            same: T::eq,
            dirty: false,
            task: TaskController::new(),
            pending: None,
            generation: None,
        });
        self
    }

    /// Start an owner-scoped read for a picker whose initial rows have not been loaded.
    pub(crate) fn refresh_on_open(mut self) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.dirty = true;
        }
        self
    }

    /// Search and VCS rows depend on file contents as well as directory membership.
    pub(crate) fn refresh_on_content_changes(mut self) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.changes.policy.contents = true;
        }
        self
    }

    /// Text search also depends on unsaved editor buffers, independently of disk events.
    pub(crate) fn refresh_on_document_changes(mut self) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.changes.policy.documents = Documents::Workspace;
        }
        self
    }

    /// Linked-worktree metadata can live outside the picker's worktree root.
    pub(crate) fn refresh_on_vcs_changes(mut self) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.changes.policy.vcs = true;
        }
        self
    }

    /// Refresh after an explicit operation even when filesystem watching and polling are disabled.
    #[cfg(feature = "git")]
    pub(crate) fn with_refresh_generation(
        mut self,
        editor: &Editor,
        generation: fn(&Editor) -> u64,
    ) -> Self {
        if let Some(updates) = &mut self.file_updates {
            updates.generation = Some(Generation {
                current: generation,
                observed: generation(editor),
            });
        }
        self
    }

    pub(super) fn refresh_files(&mut self, editor: &Editor) {
        use oneshot::error::TryRecvError as ResultError;

        let Some(mut updates) = self.file_updates.take() else {
            return;
        };
        updates.dirty |= updates
            .changes
            .changed(editor, (updates.root)(&self.results.editor_data));
        if let Some(generation) = &mut updates.generation {
            let current = (generation.current)(editor);
            if current != generation.observed {
                generation.observed = current;
                updates.stop();
                if let Some(preview) = &mut self.custom_preview {
                    preview.invalidate();
                }
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
                    updates.changes.mark_refreshed();
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

        if updates.pending.is_none() && updates.changes.poll_due(editor) {
            updates.dirty = true;
        }
        if updates.dirty
            && !self.dynamic_query_pending
            && updates.pending.is_none()
            && self.results.matcher.active_injectors() == 0
            && !self.results.is_refreshing()
        {
            updates.dirty = false;
            updates.changes.mark_refreshed();
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
