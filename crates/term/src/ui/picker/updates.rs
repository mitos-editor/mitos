//! Publish background replacements together, preserving the current interaction.

use std::{hash::Hash, sync::Arc};

use super::{visible_column_widths, Picker};
use view::Editor;

pub(super) type QueryUpdates<D> = Box<dyn FnMut(&mut Editor, &D) -> bool>;

/// Feature-owned state is sampled on redraw without rebuilding unchanged rows.
#[cfg_attr(
    not(any(feature = "git", test)),
    expect(dead_code, reason = "Loading hints are consumed by Git sources")
)]
pub(crate) struct UpdateContext<'a, T> {
    pub editor: &'a mut Editor,
    /// Search input, including column filters.
    pub query: &'a str,
    /// Selection or viewport is within one screen of the final matched row.
    pub near_end: bool,
    /// Persistent source status displayed below the search input.
    pub status: &'a mut Option<String>,
    /// Loaded rows cover only part of the source; their count is not its total.
    pub incomplete: &'a mut bool,
    pub selected: Option<&'a T>,
    pub task: &'a mut event::TaskController,
    /// A cached picker was restored and may have discarded an unpublished refresh.
    pub resumed: bool,
    pub toggle_loading: bool,
}

/// Append extends one source generation; replace publishes a refreshed snapshot.
pub(crate) enum Update<T> {
    #[cfg(any(feature = "git", test))]
    Append(Vec<T>),
    Replace(Vec<T>),
}

/// Inputs tied to the current published data and query generation.
pub(crate) struct SourceInput<'a, D> {
    pub(super) data: &'a Arc<D>,
    pub(super) version: usize,
    pub(super) query: &'a str,
    pub(super) can_load: bool,
    pub(super) progress: &'a super::source::Progress,
}

#[derive(Clone, Copy)]
pub(crate) enum Invalidation {
    /// Cancel an obsolete scan, retaining an already requested refresh.
    Query,
    /// Request another scan even when the source was idle.
    Refresh,
}

/// A source owns background activity separately from the picker's visible rows.
/// Suspension releases source resources; the next update resumes or rebuilds them.
pub(crate) trait PickerSource<T, D = ()> {
    fn update(&mut self, cx: UpdateContext<'_, T>, input: SourceInput<'_, D>) -> Option<Update<T>>;
    fn suspend(&mut self) {}
    fn invalidate(&mut self, _reason: Invalidation) {}
    fn is_pending(&self) -> bool {
        false
    }
    fn documents_refreshed(&mut self, _editor: &Editor) {}
}

impl<T, D, F: FnMut(UpdateContext<'_, T>) -> Option<Update<T>>> PickerSource<T, D> for F {
    fn update(
        &mut self,
        cx: UpdateContext<'_, T>,
        _input: SourceInput<'_, D>,
    ) -> Option<Update<T>> {
        self(cx)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceKind {
    Editor,
    Files,
    Paged,
}

struct Generation {
    current: fn(&Editor) -> u64,
    observed: u64,
}

pub(super) struct SourceUpdates<T, D> {
    source: Box<dyn PickerSource<T, D>>,
    same: fn(&T, &T) -> bool,
    pub task: event::TaskController,
    pub resumed: bool,
    pub status: Option<String>,
    pub incomplete: bool,
    pub toggle_loading: bool,
    pub kind: SourceKind,
    generation: Option<Generation>,
}

impl<T, D> SourceUpdates<T, D> {
    pub fn is_pending(&self) -> bool {
        self.source.is_pending()
    }

    pub fn invalidate(&mut self, reason: Invalidation) {
        if self.kind != SourceKind::Files {
            return;
        }
        self.task.cancel();
        self.source.invalidate(reason);
    }

    pub fn documents_refreshed(&mut self, editor: &Editor) {
        self.source.documents_refreshed(editor);
    }

    pub fn suspend(&mut self) {
        self.task.cancel();
        self.source.suspend();
        self.resumed = true;
    }
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
            .is_some_and(|refresh| refresh(editor, &self.results.editor_data));
        let resume = std::mem::take(&mut self.query_needs_refresh);
        if !(changed || resume) {
            return;
        }
        if self.async_query_options.is_some() {
            self.refresh_query_options(editor, true);
        } else {
            self.request_dynamic_refresh(false);
        }
    }

    pub(super) fn request_dynamic_refresh(&mut self, is_paste: bool) {
        let query = self.primary_query();
        let Some(dynamic_query) = &mut self.dynamic_query else {
            return;
        };
        self.results.discard_pending();
        dynamic_query.schedule(
            super::handlers::DynamicQueryChange { query, is_paste },
            Some(super::handlers::QueryKind::Refresh),
            &self.results.version,
        );
    }

    pub(super) fn refresh_injector(&mut self) -> super::Injector<T, D> {
        self.refresh_options(Vec::new(), self.results.editor_data.clone());
        let mut injector = self.injector();
        injector.dst = self
            .results
            .pending_options
            .as_ref()
            .unwrap()
            .matcher
            .injector();
        injector
    }

    pub(crate) fn with_editor_updates(
        self,
        mut refresh: impl FnMut(UpdateContext<'_, T>) -> Option<Vec<T>> + 'static,
    ) -> Self
    where
        T: PartialEq,
    {
        self.with_updates(
            move |cx: UpdateContext<'_, T>| refresh(cx).map(Update::Replace),
            SourceKind::Editor,
        )
    }

    #[cfg(any(feature = "git", test))]
    pub(crate) fn with_paged_updates(self, source: impl PickerSource<T, D> + 'static) -> Self
    where
        T: PartialEq,
    {
        self.with_updates(source, SourceKind::Paged)
    }

    pub(super) fn with_updates(
        mut self,
        source: impl PickerSource<T, D> + 'static,
        kind: SourceKind,
    ) -> Self
    where
        T: PartialEq,
    {
        self.updates = Some(SourceUpdates {
            source: Box::new(source),
            same: T::eq,
            task: event::TaskController::new(),
            resumed: false,
            status: None,
            incomplete: false,
            toggle_loading: false,
            kind,
            generation: None,
        });
        self
    }

    pub(super) fn has_file_source(&self) -> bool {
        self.updates
            .as_ref()
            .is_some_and(|updates| updates.kind == SourceKind::Files)
    }

    pub(super) fn invalidate_source(&mut self, reason: Invalidation) {
        if let Some(updates) = &mut self.updates {
            updates.invalidate(reason);
        }
    }

    pub(super) fn source_pending(&self) -> bool {
        self.updates.as_ref().is_some_and(SourceUpdates::is_pending)
    }

    /// Start an owner-scoped read when the initial rows have not been loaded.
    #[cfg(feature = "git")]
    pub(crate) fn refresh_on_open(mut self) -> Self {
        self.invalidate_source(Invalidation::Refresh);
        self
    }

    /// Explicit operations invalidate a source even with watching and polling disabled.
    pub(crate) fn with_refresh_generation(
        mut self,
        editor: &Editor,
        generation: fn(&Editor) -> u64,
    ) -> Self {
        if let Some(updates) = &mut self.updates {
            updates.generation = Some(Generation {
                current: generation,
                observed: generation(editor),
            });
        }
        self
    }

    pub(super) fn refresh_source(&mut self, editor: &mut Editor) {
        if self.has_file_source() {
            if self.replacement_pending() {
                return;
            }
        } else if self.results.pending_options.is_some() || self.results.tick(0).running {
            // Matching can consume its redraw signal before this deferred refresh runs.
            if self.updates.is_some() {
                self.redraw.request();
            }
            return;
        }
        let Some(mut updates) = self.updates.take() else {
            return;
        };
        if let Some(generation) = &mut updates.generation {
            let current = (generation.current)(editor);
            if current != generation.observed {
                generation.observed = current;
                updates.invalidate(Invalidation::Refresh);
                if let Some(preview) = &mut self.custom_preview {
                    preview.invalidate();
                }
            }
        }
        let query = self.primary_query();
        let input = SourceInput {
            data: &self.results.editor_data,
            version: self
                .results
                .version
                .load(std::sync::atomic::Ordering::Relaxed),
            query: &query,
            can_load: self.pending_query.is_none()
                && !self.dynamic_query_pending()
                && self.results.matcher.active_injectors() == 0
                && !self.results.is_refreshing(),
            progress: &self.results.source,
        };
        let resumed = std::mem::take(&mut updates.resumed);
        if let Some(update) = updates.source.update(
            UpdateContext {
                editor,
                query: self.prompt.line(),
                near_end: {
                    let count = self.results.matcher.snapshot().matched_item_count();
                    let height = u32::from(self.completion_height);
                    count > 0
                        && self
                            .results
                            .cursor
                            .saturating_add(1)
                            .max(self.results.offset.saturating_add(height))
                            .saturating_add(height)
                            >= count
                },
                status: &mut updates.status,
                incomplete: &mut updates.incomplete,
                selected: self.selection(),
                task: &mut updates.task,
                resumed,
                toggle_loading: std::mem::take(&mut updates.toggle_loading),
            },
            input,
        ) {
            self.results.tick(0);
            #[cfg(any(feature = "git", test))]
            let options = match update {
                #[cfg(any(feature = "git", test))]
                Update::Append(rows) => {
                    self.results.append(rows, &self.columns);
                    self.updates = Some(updates);
                    return;
                }
                Update::Replace(rows) => rows,
            };
            #[cfg(not(any(feature = "git", test)))]
            let Update::Replace(options) = update;
            let snapshot = self.results.matcher.snapshot();
            let unchanged = snapshot.item_count() as usize == options.len()
                && options.iter().enumerate().all(|(index, item)| {
                    snapshot
                        .get_item(index as u32)
                        .is_some_and(|old| (updates.same)(old.data, item))
                });
            if !unchanged {
                self.refresh_options(options, self.results.editor_data.clone());
            }
        }
        self.updates = Some(updates);
    }

    /// Load immutable options under this picker's lifecycle. Hidden/cached pickers
    /// discard pending results and retry on restore; published rows remain stable.
    #[cfg(any(feature = "git", test))]
    pub(crate) fn with_initial_options(
        self,
        load: impl Fn(
                event::TaskHandle,
            ) -> futures_util::future::BoxFuture<'static, anyhow::Result<Vec<T>>>
            + 'static,
    ) -> Self
    where
        T: PartialEq,
    {
        let mut pages = super::paged::Paged::new(
            Some(()),
            ["Results available", "Loading results…", "Complete results"],
            |_| std::mem::size_of::<T>(),
            move |(), cancel| {
                let work = load(cancel);
                Box::pin(async move {
                    Ok(super::paged::Page {
                        rows: work.await?,
                        next: None,
                    })
                })
            },
        );
        self.with_paged_updates(move |cx: UpdateContext<'_, T>| {
            pages.update(cx).map(|update| match update {
                Update::Append(rows) => Update::Replace(rows),
                update => update,
            })
        })
    }

    /// Identify rows independently of their displayed metadata. If the selected row
    /// disappears, prefer its next surviving neighbour, then its previous one.
    pub(crate) fn with_item_key<K: Eq + Hash + 'static>(
        mut self,
        key: impl Fn(&T) -> K + 'static,
    ) -> Self {
        self.results.set_item_key(key);
        self
    }

    pub(crate) fn refresh_options(&mut self, options: Vec<T>, data: Arc<D>) {
        self.results.refresh(
            options,
            data,
            &self.columns,
            self.file_fn.is_some(),
            &self.redraw,
        );
    }

    pub(super) fn publish_options(&mut self) {
        if self.results.publish() {
            self.widths = visible_column_widths(&self.columns);
        }
    }
}

#[cfg(test)]
mod initial_options_tests {
    use super::*;
    use crate::{
        compositor::Context,
        keymap::component::PickerAction,
        ui::{
            picker::test::{application, draw, until},
            PickerColumn,
        },
    };

    #[tokio::test]
    async fn failed_initial_load_remains_partial_and_can_retry_without_reopening(
    ) -> anyhow::Result<()> {
        let mut app = application(false)?;
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [PickerColumn::new("item", |item: &u32, _: &()| {
                item.to_string().into()
            })],
            0,
            [],
            (),
            |_, _, _| {},
        )
        .with_initial_options({
            let attempts = std::sync::atomic::AtomicUsize::new(0);
            move |_| {
                let failed = attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;
                Box::pin(async move {
                    anyhow::ensure!(!failed, "initial read failed");
                    Ok(vec![7])
                })
            }
        });
        until(&mut picker, &mut app, |p| {
            p.updates.as_ref().is_some_and(|updates| {
                updates.status.as_deref() == Some("Results incomplete; retry loading")
            })
        })
        .await?;
        assert!(picker.updates.as_ref().unwrap().incomplete);
        picker.updates.as_mut().unwrap().toggle_loading = true;
        until(&mut picker, &mut app, |p| p.selection() == Some(&7)).await?;
        assert!(!picker.updates.as_ref().unwrap().incomplete);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hiding_and_restoring_a_picker_rejects_the_previous_initial_read() -> anyhow::Result<()>
    {
        let mut app = application(false)?;
        let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [PickerColumn::new("item", |item: &u32, _: &()| {
                item.to_string().into()
            })],
            0,
            [],
            (),
            |_, _, _| {},
        )
        .with_initial_options(move |cancel| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            started.send((cancel, tx)).unwrap();
            Box::pin(async move { rx.await? })
        });
        let (old, old_result) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                draw(&mut picker, &mut app);
                if let Ok(request) = requests.try_recv() {
                    break request;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await?;
        let config = crate::config::Config::default();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut jobs = crate::job::Jobs::new();
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: true,
        };
        let _ = picker.execute_actions(&[PickerAction::Cancel], &mut cx);
        assert!(old.is_canceled());
        let _ = old_result.send(Ok(vec![1]));
        let (current, current_result) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    draw(&mut picker, &mut app);
                    if let Ok(request) = requests.try_recv() {
                        break request;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await?;
        assert!(!current.is_canceled());
        current_result.send(Ok(vec![2])).unwrap();
        until(&mut picker, &mut app, |picker| {
            picker.selection() == Some(&2)
        })
        .await?;
        assert_eq!(picker.results.matcher.snapshot().item_count(), 1);
        Ok(())
    }
}

#[cfg(test)]
mod deferred_refresh_tests {
    use super::*;
    use crate::ui::{
        picker::{
            test::{application, draw},
            PickerCallbackResult,
        },
        PickerColumn,
    };
    use futures_util::FutureExt;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test(flavor = "multi_thread")]
    async fn initial_load_deferred_by_matching_finishes_without_another_keypress(
    ) -> anyhow::Result<()> {
        let mut app = application(false)?;
        // The matcher completion signal has already been consumed by the current
        // frame. The source coordinator must arrange its own continuation.
        let matcher = || nucleo::Nucleo::new(nucleo::Config::DEFAULT, Arc::new(|| {}), Some(1), 1);
        let mut picker = Picker::with(
            app.editor.redraw().clone(),
            matcher(),
            Arc::new([PickerColumn::new("item", |item: &u32, _: &()| {
                item.to_string().into()
            })]),
            0,
            Arc::new(()),
            Arc::new(AtomicUsize::new(0)),
            |_, _, _| PickerCallbackResult::Close,
        )
        .with_initial_options(|_| Box::pin(async { Ok(vec![7]) }));
        picker.results.spare_matcher = Some(matcher());
        picker.refresh_options(vec![0], Arc::new(()));
        app.editor.redraw().start_frame();
        picker.refresh_source(&mut app.editor);
        assert!(
            app.editor.redraw().requested().now_or_never().is_some(),
            "a deferred source refresh must schedule a follow-up frame"
        );
        draw(&mut picker, &mut app);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while picker.selection() != Some(&7) {
                app.editor.redraw().requested().await;
                draw(&mut picker, &mut app);
            }
        })
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod source_lifecycle_tests {
    use super::*;
    use crate::ui::{
        picker::{
            paged::{Page, Paged},
            test::{application, draw, until},
        },
        PickerColumn,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct Cursor {
        offset: u32,
        suspended: Arc<AtomicUsize>,
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn suspending_paged_source_discards_late_rows_and_preserves_selection(
    ) -> anyhow::Result<()> {
        let mut app = application(false)?;
        let suspended = Arc::new(AtomicUsize::new(0));
        let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let pages = Paged::new(
            Some(Cursor {
                offset: 0,
                suspended: suspended.clone(),
            }),
            ["More", "Loading", "Complete"],
            |_| 4,
            move |cursor: Cursor, cancel| {
                let (tx, rx) = tokio::sync::oneshot::channel();
                started.send((cursor, cancel, tx)).unwrap();
                Box::pin(async move { Ok(rx.await?) })
            },
        )
        .with_suspend(|cursor| {
            cursor.suspended.fetch_add(1, Ordering::Relaxed);
        });
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [PickerColumn::new("item", |item: &u32, _: &()| {
                item.to_string().into()
            })],
            0,
            [],
            (),
            |_, _, _| {},
        )
        .with_paged_updates(pages);
        let (initial, _, result) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                draw(&mut picker, &mut app);
                if let Ok(request) = requests.try_recv() {
                    break request;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        result
            .send(Page {
                rows: vec![1],
                next: Some(Cursor {
                    offset: 1,
                    ..initial
                }),
            })
            .ok()
            .unwrap();
        until(&mut picker, &mut app, |p| p.selection() == Some(&1)).await?;
        let (old_cursor, old_cancel, old_result) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    draw(&mut picker, &mut app);
                    if let Ok(request) = requests.try_recv() {
                        break request;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await?;
        assert_eq!(old_cursor.offset, 1);
        picker.suspend();
        assert!(old_cancel.is_canceled());
        assert_eq!(suspended.load(Ordering::Relaxed), 1);
        let _ = old_result.send(Page {
            rows: vec![99],
            next: None,
        });
        draw(&mut picker, &mut app);
        let (fresh, cancel, result) =
            tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
                .await?
                .unwrap();
        assert!(!cancel.is_canceled());
        assert_eq!(fresh.offset, 1);
        result
            .send(Page {
                rows: vec![2],
                next: None,
            })
            .ok()
            .unwrap();
        until(&mut picker, &mut app, |p| {
            p.results.matcher.snapshot().item_count() == 2
        })
        .await?;
        assert_eq!(picker.selection(), Some(&1));
        let snapshot = picker.results.matcher.snapshot();
        assert_eq!(*snapshot.get_item(0).unwrap().data, 1);
        assert_eq!(*snapshot.get_item(1).unwrap().data, 2);
        Ok(())
    }
}
