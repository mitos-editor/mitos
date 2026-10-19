//! Publish background replacements together, preserving the current interaction.

use std::{collections::HashMap, hash::Hash, sync::Arc};

use nucleo::{Config, Nucleo, Snapshot};

use super::{inject_nucleo_item, visible_column_widths, Picker};
use view::Editor;

pub(super) type SelectionResolver<T> = Box<dyn Fn(&Snapshot<T>, &Snapshot<T>, u32) -> Option<u32>>;
pub(super) type QueryUpdates<D> = Box<dyn FnMut(&mut Editor, &D) -> bool>;

pub(super) struct PendingOptions<T: Send + Sync + 'static, D> {
    pub matcher: Nucleo<T>,
    data: Arc<D>,
    pub preserve_selection: bool,
}

/// Feature-owned state is sampled on redraw without rebuilding unchanged rows.
pub(crate) struct UpdateContext<'a, T> {
    pub editor: &'a mut Editor,
    pub selected: Option<&'a T>,
    pub task: &'a mut event::TaskController,
}

type EditorRefresh<T> = Box<dyn FnMut(UpdateContext<'_, T>) -> Option<Vec<T>>>;

pub(super) struct EditorUpdates<T> {
    refresh: EditorRefresh<T>,
    same: fn(&T, &T) -> bool,
    pub task: event::TaskController,
}

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    pub(crate) fn with_query_updates(
        mut self,
        refresh: impl FnMut(&mut Editor, &D) -> bool + 'static,
    ) -> Self {
        self.query_updates = Some(Box::new(refresh));
        self
    }

    pub(super) fn refresh_query_if_changed(&mut self, editor: &mut Editor) {
        let changed = self
            .query_updates
            .as_mut()
            .is_some_and(|refresh| refresh(editor, &self.editor_data));
        let resume = std::mem::take(&mut self.query_needs_refresh);
        if !(changed || resume) {
            return;
        }
        let Some(handler) = &self.dynamic_query_handler else {
            return;
        };
        self.query_refresh = true;
        self.pending_options = None;
        self.refreshing = false;
        self.dynamic_query_request.cancel();
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        event::send_blocking(
            handler,
            super::handlers::DynamicQueryChange {
                query: self.primary_query(),
                is_paste: false,
            },
        );
    }

    pub(super) fn refresh_injector(&mut self) -> super::Injector<T, D> {
        self.refresh_options(Vec::new(), self.editor_data.clone());
        let mut injector = self.injector();
        injector.dst = self.pending_options.as_ref().unwrap().matcher.injector();
        injector
    }

    pub(crate) fn with_editor_updates(
        mut self,
        refresh: impl FnMut(UpdateContext<'_, T>) -> Option<Vec<T>> + 'static,
    ) -> Self
    where
        T: PartialEq,
    {
        self.editor_updates = Some(EditorUpdates {
            refresh: Box::new(refresh),
            same: T::eq,
            task: event::TaskController::new(),
        });
        self
    }

    pub(super) fn refresh_from_editor(&mut self, editor: &mut Editor) {
        if self.pending_options.is_some() {
            return;
        }
        if self.matcher.tick(0).running {
            return;
        }
        let Some(mut updates) = self.editor_updates.take() else {
            return;
        };
        if let Some(options) = (updates.refresh)(UpdateContext {
            editor,
            selected: self.selection(),
            task: &mut updates.task,
        }) {
            let snapshot = self.matcher.snapshot();
            let unchanged = snapshot.item_count() as usize == options.len()
                && options.iter().enumerate().all(|(index, item)| {
                    snapshot
                        .get_item(index as u32)
                        .is_some_and(|old| (updates.same)(old.data, item))
                });
            if !unchanged {
                self.refresh_options(options, self.editor_data.clone());
            }
        }
        self.editor_updates = Some(updates);
    }

    /// Identify rows independently of their displayed metadata. If the selected row
    /// disappears, prefer its next surviving neighbour, then its previous one.
    pub(crate) fn with_item_key<K: Eq + Hash + 'static>(
        mut self,
        key: impl Fn(&T) -> K + 'static,
    ) -> Self {
        self.selection_resolver = Some(Box::new(move |old, new, cursor| {
            let mut indices: HashMap<_, Vec<u32>> = HashMap::new();
            for (index, item) in new
                .matched_items(0..new.matched_item_count())
                .enumerate()
                .rev()
            {
                indices
                    .entry(key(item.data))
                    .or_default()
                    .push(index as u32);
            }
            // Equal symbol names can occur more than once. Match their relative
            // occurrence order instead of collapsing them onto the last row.
            let positions: Vec<_> = old
                .matched_items(0..old.matched_item_count())
                .map(|item| indices.get_mut(&key(item.data)).and_then(Vec::pop))
                .collect();
            let cursor = cursor.min(old.matched_item_count());
            (cursor..old.matched_item_count())
                .chain((0..cursor).rev())
                .find_map(|index| positions[index as usize])
        }));
        self
    }

    pub(crate) fn refresh_options(&mut self, options: Vec<T>, data: Arc<D>) {
        let config = if self.file_fn.is_some() {
            Config::DEFAULT.match_paths()
        } else {
            Config::DEFAULT
        };
        let mut matcher = self.spare_matcher.take().unwrap_or_else(|| {
            Nucleo::new(
                config,
                Arc::new(self.redraw.callback()),
                None,
                self.columns.iter().filter(|column| column.filter).count() as u32,
            )
        });
        matcher.restart(true);
        matcher.pattern.clone_from(&self.matcher.pattern);
        let injector = matcher.injector();
        for item in options {
            inject_nucleo_item(&injector, &self.columns, item, &data);
        }
        self.refreshing = true;
        self.pending_options = Some(PendingOptions {
            matcher,
            data,
            preserve_selection: true,
        });
        self.redraw.request();
    }

    pub(super) fn publish_options(&mut self) {
        let Some(pending) = &mut self.pending_options else {
            return;
        };
        if pending.matcher.active_injectors() > 0 {
            return;
        }
        if pending.matcher.tick(0).running {
            return;
        }
        let pending = self.pending_options.take().unwrap();
        // Resolve at publication, so navigation during the refresh wins.
        let cursor = self
            .selection_resolver
            .as_ref()
            .filter(|_| pending.preserve_selection)
            .and_then(|resolve| {
                resolve(
                    self.matcher.snapshot(),
                    pending.matcher.snapshot(),
                    self.cursor,
                )
            })
            .unwrap_or(self.cursor)
            .min(
                pending
                    .matcher
                    .snapshot()
                    .matched_item_count()
                    .saturating_sub(1),
            );
        self.offset = (i64::from(self.offset) + i64::from(cursor) - i64::from(self.cursor))
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.cursor = cursor;
        self.spare_matcher = Some(std::mem::replace(&mut self.matcher, pending.matcher));
        self.editor_data = pending.data;
        self.widths = visible_column_widths(&self.columns);
        self.refreshing = false;
        // Preview invalidation follows document/filesystem changes, not row changes.
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
