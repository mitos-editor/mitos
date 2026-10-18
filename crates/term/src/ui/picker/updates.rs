//! Publish background replacements together, preserving the current interaction.

use std::{collections::HashMap, hash::Hash, sync::Arc};

use nucleo::{Config, Nucleo, Snapshot};

use super::{inject_nucleo_item, visible_column_widths, Picker};

pub(super) type SelectionResolver<T> = Box<dyn Fn(&Snapshot<T>, &Snapshot<T>, u32) -> Option<u32>>;

pub(super) struct PendingOptions<T: Send + Sync + 'static, D> {
    matcher: Nucleo<T>,
    data: Arc<D>,
}

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    /// Identify rows independently of their displayed metadata. If the selected row
    /// disappears, prefer its next surviving neighbour, then its previous one.
    pub(crate) fn with_item_key<K: Eq + Hash + 'static>(
        mut self,
        key: impl Fn(&T) -> K + 'static,
    ) -> Self {
        self.selection_resolver = Some(Box::new(move |old, new, cursor| {
            let indices: HashMap<_, _> = new
                .matched_items(0..new.matched_item_count())
                .enumerate()
                .map(|(index, item)| (key(item.data), index as u32))
                .collect();
            let cursor = cursor.min(old.matched_item_count());
            (cursor..old.matched_item_count())
                .chain((0..cursor).rev())
                .find_map(|index| {
                    indices
                        .get(&key(old.get_matched_item(index)?.data))
                        .copied()
                })
        }));
        self
    }

    pub(super) fn refresh_options(&mut self, options: Vec<T>, data: Arc<D>) {
        let config = if self.file_fn.is_some() {
            Config::DEFAULT.match_paths()
        } else {
            Config::DEFAULT
        };
        let mut matcher = Nucleo::new(
            config,
            Arc::new(self.redraw.callback()),
            None,
            self.columns.iter().filter(|column| column.filter).count() as u32,
        );
        matcher.pattern.clone_from(&self.matcher.pattern);
        let injector = matcher.injector();
        for item in options {
            inject_nucleo_item(&injector, &self.columns, item, &data);
        }
        self.refreshing = true;
        self.pending_options = Some(PendingOptions { matcher, data });
        self.redraw.request();
    }

    pub(super) fn publish_options(&mut self) {
        let Some(pending) = &mut self.pending_options else {
            return;
        };
        if pending.matcher.tick(0).running {
            return;
        }
        let pending = self.pending_options.take().unwrap();
        // Resolve at publication, so navigation during the refresh wins.
        let cursor = self
            .selection_resolver
            .as_ref()
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
        self.matcher = pending.matcher;
        self.editor_data = pending.data;
        self.widths = visible_column_widths(&self.columns);
        self.refreshing = false;
        // Preview invalidation follows document/filesystem changes, not row changes.
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
