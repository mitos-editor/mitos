//! Debounced filesystem refreshes that keep the current results visible while loading.

use std::{path::Path, sync::Arc};

use event::TaskHandle;
use futures_util::future::BoxFuture;
use tokio::sync::oneshot;
use view::{
    features::workspace_changes::{Policy, WorkspaceChanges},
    Editor,
};

use super::{
    updates::{Invalidation, PickerSource, SourceInput, SourceKind, Update, UpdateContext},
    Picker,
};

type Refresh<T, D> =
    fn(&str, &Editor, Arc<D>, TaskHandle) -> BoxFuture<'static, anyhow::Result<Vec<T>>>;

pub(super) struct FileUpdates<T, D> {
    changes: WorkspaceChanges,
    root: fn(&D) -> &Path,
    refresh: Refresh<T, D>,
    dirty: bool,
    pending: Option<Pending<T, D>>,
}

struct Pending<T, D> {
    version: usize,
    data: Arc<D>,
    result: oneshot::Receiver<anyhow::Result<Vec<T>>>,
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Picker<T, D> {
    pub(crate) fn with_file_updates(
        mut self,
        editor: &Editor,
        root: fn(&D) -> &Path,
        refresh: Refresh<T, D>,
        mut policy: Policy,
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
        policy.ignore_files = true;
        let source = FileUpdates {
            changes: WorkspaceChanges::new(editor, root(&self.results.editor_data), policy),
            root,
            refresh,
            dirty: false,
            pending: None,
        };
        self.with_updates(source, SourceKind::Files)
    }
}

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> PickerSource<T, D> for FileUpdates<T, D> {
    fn is_pending(&self) -> bool {
        self.dirty || self.pending.is_some()
    }

    fn documents_refreshed(&mut self, editor: &Editor) {
        self.changes.mark_documents_refreshed(editor);
    }

    fn invalidate(&mut self, reason: Invalidation) {
        self.dirty |= self.pending.take().is_some() || matches!(reason, Invalidation::Refresh);
    }

    fn suspend(&mut self) {
        self.invalidate(Invalidation::Refresh);
    }

    fn update(&mut self, cx: UpdateContext<'_, T>, input: SourceInput<'_, D>) -> Option<Update<T>> {
        use oneshot::error::TryRecvError as ResultError;
        let updates = self;
        let editor = &*cx.editor;
        updates.dirty |= updates.changes.changed(editor, (updates.root)(input.data));
        let mut rows = None;
        if updates.pending.as_ref().is_some_and(|pending| {
            !Arc::ptr_eq(&pending.data, input.data) || pending.version != input.version
        }) {
            cx.task.cancel();
            updates.invalidate(Invalidation::Refresh);
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
                            input.progress.set(super::source::State::Complete);
                            rows = Some(Update::Replace(options));
                        }
                        Err(error) => {
                            input.progress.set(super::source::State::Failed);
                            editor.status_sender().report_blocking(error);
                        }
                    }
                }
                Err(ResultError::Closed) => updates.pending = None,
                Err(ResultError::Empty) => {}
            }
        }

        // The shared publisher must install these rows before a follow-up scan
        // captures its generation. Unchanged rows still need another update.
        if rows.is_some() {
            if updates.dirty {
                editor.redraw().request();
            }
            return rows;
        }
        if updates.pending.is_none() && updates.changes.poll_due(editor) {
            updates.dirty = true;
        }
        if updates.dirty && input.can_load && updates.pending.is_none() {
            updates.dirty = false;
            updates.changes.mark_refreshed();
            let cancel = cx.task.restart();
            let load = (updates.refresh)(input.query, editor, input.data.clone(), cancel.clone());
            let (tx, result) = oneshot::channel();
            updates.pending = Some(Pending {
                version: input.version,
                data: input.data.clone(),
                result,
            });
            let redraw = editor.redraw().clone();
            tokio::spawn(async move {
                let work = async {
                    let result = load.await;
                    let _ = tx.send(result);
                    redraw.request();
                };
                event::cancelable_future(work, cancel).await;
            });
        }
        rows
    }
}

#[cfg(test)]
#[path = "file_updates_tests.rs"]
mod tests;
