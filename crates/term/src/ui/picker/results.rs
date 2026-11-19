//! Active and pending result sets, publication, and navigation state.
//!
//! A replacement is matched before publication. Selection is resolved at publication
//! so navigation during a refresh remains authoritative.

use super::{inject_nucleo_item, Column};
use nucleo::{Config, Nucleo, Snapshot};
use std::{
    collections::HashMap,
    hash::Hash,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

type SelectionResolver<T> = Box<dyn Fn(&Snapshot<T>, &Snapshot<T>, u32) -> Option<u32>>;

pub(super) struct PendingOptions<T: Send + Sync + 'static, D> {
    pub matcher: Nucleo<T>,
    data: Arc<D>,
    pub preserve_selection: bool,
    /// A query's initial selection belongs to the replacement rows.
    pub cursor: Option<u32>,
}

pub(super) struct Results<T: Send + Sync + 'static, D> {
    pub matcher: Nucleo<T>,
    pub spare_matcher: Option<Nucleo<T>>,
    pub editor_data: Arc<D>,
    pub version: Arc<AtomicUsize>,
    pub cursor: u32,
    pub offset: u32,
    pub preserve_selection: bool,
    pub source: super::source::Progress,
    pub pending_options: Option<PendingOptions<T, D>>,
    pub selection_resolver: Option<SelectionResolver<T>>,
}

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Results<T, D> {
    pub fn new(matcher: Nucleo<T>, editor_data: Arc<D>, version: Arc<AtomicUsize>) -> Self {
        Self {
            matcher,
            editor_data,
            version,
            cursor: 0,
            offset: 0,
            preserve_selection: true,
            source: Default::default(),
            spare_matcher: None,
            pending_options: None,
            selection_resolver: None,
        }
    }

    pub fn is_refreshing(&self) -> bool {
        self.pending_options.is_some()
    }

    pub fn discard_pending(&mut self) {
        self.pending_options = None;
    }

    pub fn invalidate(&mut self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reparse(&mut self, column: usize, pattern: &str, append: bool) {
        self.preserve_selection = false;
        use nucleo::pattern::{CaseMatching, Normalization};
        self.matcher.pattern.reparse(
            column,
            pattern,
            CaseMatching::Smart,
            Normalization::Smart,
            append,
        );
        if let Some(pending) = &mut self.pending_options {
            pending.matcher.pattern.reparse(
                column,
                pattern,
                CaseMatching::Smart,
                Normalization::Smart,
                append,
            );
        }
    }

    pub fn tick(&mut self, timeout: u64) -> nucleo::Status {
        let selected = self.append_selection();
        let status = self.matcher.tick(timeout);
        if status.changed {
            self.restore_append_selection(selected);
        }
        self.preserve_selection |= !status.running;
        status
    }

    #[cfg(any(feature = "git", test))]
    pub fn append(&mut self, rows: Vec<T>, columns: &[Column<T, D>]) {
        if rows.is_empty() {
            return;
        }
        debug_assert!(self.pending_options.is_none());
        let injector = self.matcher.injector();
        for row in rows {
            inject_nucleo_item(&injector, columns, row, &self.editor_data);
        }
    }

    /// Appending can reorder fuzzy matches. Nucleo keeps injected rows at stable
    /// addresses; capture only their identity, never dereference a saved pointer.
    pub fn append_selection(&self) -> Option<usize> {
        self.preserve_selection
            .then(|| self.matcher.snapshot().get_matched_item(self.cursor))
            .flatten()
            .map(|item| std::ptr::from_ref(item.data) as usize)
    }

    pub fn restore_append_selection(&mut self, selected: Option<usize>) {
        if let Some(selected) = selected
            && let Some(cursor) = self
                .matcher
                .snapshot()
                .matched_items(0..self.matcher.snapshot().matched_item_count())
                .position(|item| std::ptr::from_ref(item.data) as usize == selected)
        {
            let cursor = cursor as u32;
            self.offset = (i64::from(self.offset) + i64::from(cursor) - i64::from(self.cursor))
                .clamp(0, i64::from(u32::MAX)) as u32;
            self.cursor = cursor;
        }
    }

    pub fn reset(&mut self, options: Vec<T>, editor_data: Arc<D>, columns: &[Column<T, D>]) {
        // Cancel existing injectors before replacing the matcher contents.
        self.discard_pending();
        self.invalidate();
        self.preserve_selection = false;
        self.matcher.restart(true);
        self.editor_data = editor_data;

        let injector = self.matcher.injector();
        for item in options {
            inject_nucleo_item(&injector, columns, item, &self.editor_data);
        }
    }

    pub fn set_item_key<K: Eq + Hash + 'static>(&mut self, key: impl Fn(&T) -> K + 'static) {
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
    }

    pub fn refresh(
        &mut self,
        options: Vec<T>,
        data: Arc<D>,
        columns: &[Column<T, D>],
        match_paths: bool,
        redraw: &event::Redraw,
    ) {
        let injector = self.prepare_refresh(data.clone(), columns, match_paths, redraw);
        for item in options {
            inject_nucleo_item(&injector, columns, item, &data);
        }
    }

    /// Prepare an atomic replacement whose rows may be injected on a worker.
    pub fn prepare_refresh(
        &mut self,
        data: Arc<D>,
        columns: &[Column<T, D>],
        match_paths: bool,
        redraw: &event::Redraw,
    ) -> nucleo::Injector<T> {
        let config = if match_paths {
            Config::DEFAULT.match_paths()
        } else {
            Config::DEFAULT
        };
        let mut matcher = self.spare_matcher.take().unwrap_or_else(|| {
            Nucleo::new(
                config,
                Arc::new(redraw.callback()),
                None,
                columns.iter().filter(|column| column.filter).count() as u32,
            )
        });
        matcher.restart(true);
        matcher.pattern.clone_from(&self.matcher.pattern);
        let injector = matcher.injector();
        self.pending_options = Some(PendingOptions {
            matcher,
            data,
            preserve_selection: true,
            cursor: None,
        });
        redraw.request();
        injector
    }

    pub fn publish(&mut self) -> bool {
        let Some(pending) = &mut self.pending_options else {
            return false;
        };
        if pending.matcher.active_injectors() > 0 {
            return false;
        }
        if pending.matcher.tick(0).running {
            return false;
        }
        let pending = self.pending_options.take().unwrap();
        self.preserve_selection = true;
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
            .or(pending.cursor)
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
        // Preview invalidation follows document/filesystem changes, not row changes.
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        true
    }
}

impl<T: Send + Sync + 'static, D> Drop for Results<T, D> {
    fn drop(&mut self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> [Column<String, ()>; 1] {
        [Column::new("text", |item: &String, _: &()| {
            item.clone().into()
        })]
    }

    fn results() -> Results<String, ()> {
        Results::new(
            Nucleo::new(Config::DEFAULT, Arc::new(|| {}), Some(1), 1),
            Arc::new(()),
            Arc::new(AtomicUsize::new(0)),
        )
    }

    async fn matched(results: &mut Results<String, ()>) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while results.matcher.tick(10).running {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn published(results: &mut Results<String, ()>) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !results.publish() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn publication_preserves_duplicate_identity_and_latest_navigation() {
        let mut results = results();
        let columns = columns();
        results.set_item_key(|item| item.clone());
        results.reset(
            ["a", "b", "b", "c"].map(String::from).into(),
            Arc::new(()),
            &columns,
        );
        matched(&mut results).await;
        results.cursor = 1;
        results.offset = 1;
        results.refresh(
            ["x", "a", "b", "b", "c"].map(String::from).into(),
            Arc::new(()),
            &columns,
            false,
            &event::Redraw::default(),
        );
        assert_eq!(results.matcher.snapshot().item_count(), 4);
        results.cursor = 2;
        published(&mut results).await;
        assert_eq!(results.cursor, 3);
        assert_eq!(results.offset, 2);
        assert_eq!(
            results
                .matcher
                .snapshot()
                .get_matched_item(results.cursor)
                .unwrap()
                .data,
            "b"
        );
    }

    #[tokio::test]
    async fn publication_applies_the_new_query_cursor_only_with_its_rows() {
        let mut results = results();
        let columns = columns();
        results.reset(vec!["old".into()], Arc::new(()), &columns);
        matched(&mut results).await;
        results.refresh(
            ["parent", "nested", "match"].map(String::from).into(),
            Arc::new(()),
            &columns,
            false,
            &event::Redraw::default(),
        );
        let pending = results.pending_options.as_mut().unwrap();
        pending.preserve_selection = false;
        pending.cursor = Some(2);
        assert_eq!(results.cursor, 0);
        assert_eq!(results.matcher.snapshot().matched_item_count(), 1);
        published(&mut results).await;
        assert_eq!(results.cursor, 2);
        assert_eq!(
            results
                .matcher
                .snapshot()
                .get_matched_item(results.cursor)
                .unwrap()
                .data,
            "match"
        );
    }

    #[tokio::test]
    async fn publication_waits_for_streaming_replacement_to_finish() {
        let mut results = results();
        let columns = columns();
        results.reset(vec!["old".into()], Arc::new(()), &columns);
        matched(&mut results).await;
        results.refresh(
            Vec::new(),
            Arc::new(()),
            &columns,
            false,
            &event::Redraw::default(),
        );
        let injector = results.pending_options.as_ref().unwrap().matcher.injector();
        inject_nucleo_item(&injector, &columns, "new".into(), &());
        assert!(!results.publish());
        assert_eq!(results.matcher.snapshot().get_item(0).unwrap().data, "old");
        drop(injector);
        published(&mut results).await;
        assert_eq!(results.matcher.snapshot().get_item(0).unwrap().data, "new");
    }

    #[test]
    fn dropping_result_owner_invalidates_producers() {
        let results = results();
        let version = results.version.clone();
        drop(results);
        assert_eq!(version.load(Ordering::Relaxed), 1);
    }
}

#[cfg(test)]
mod append_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static FORMATTED: AtomicUsize = AtomicUsize::new(0);
    fn columns() -> [Column<String, ()>; 1] {
        [Column::new("text", |item: &String, _: &()| {
            FORMATTED.fetch_add(1, Ordering::Relaxed);
            item.clone().into()
        })]
    }
    fn results() -> Results<String, ()> {
        Results::new(
            Nucleo::new(Config::DEFAULT, Arc::new(|| {}), Some(1), 1),
            Arc::new(()),
            Arc::new(AtomicUsize::new(0)),
        )
    }
    fn settle(r: &mut Results<String, ()>) {
        loop {
            let status = r.tick(10);
            if !status.running {
                break;
            }
        }
    }

    #[test]
    fn append_formats_only_new_rows_and_preserves_selection_when_matches_reorder() {
        FORMATTED.store(0, Ordering::Relaxed);
        let mut r = results();
        r.append(vec!["axbyc".into(), "a___b___c".into()], &columns());
        settle(&mut r);
        r.reparse(0, "abc", false);
        settle(&mut r);
        r.cursor = 1;
        r.offset = 1;
        let selected = r
            .matcher
            .snapshot()
            .get_matched_item(1)
            .unwrap()
            .data
            .clone();
        r.append(vec!["abc".into()], &columns());
        settle(&mut r);
        assert_eq!(FORMATTED.load(Ordering::Relaxed), 3);
        assert_eq!(
            r.matcher
                .snapshot()
                .get_matched_item(r.cursor)
                .unwrap()
                .data,
            &selected
        );
        assert_eq!(r.cursor, 2);
        assert_eq!(r.offset, 2);
    }

    /// Run explicitly to compare cumulative append cost at increasing sizes.
    #[test]
    #[ignore = "manual picker scaling measurement"]
    fn append_scaling() {
        for count in [5_000, 50_000, 100_000] {
            let mut r = results();
            let start = std::time::Instant::now();
            for offset in (0..count).step_by(100) {
                r.append(
                    (offset..offset + 100)
                        .map(|n| format!("commit-{n:08} author subject"))
                        .collect(),
                    &columns(),
                );
                settle(&mut r);
            }
            eprintln!("{count} rows: {:?}", start.elapsed());
            assert_eq!(r.matcher.snapshot().item_count(), count);
        }
    }
}
