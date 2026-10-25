//! Publish background replacements together, preserving the current interaction.

use std::{hash::Hash, sync::Arc};

use super::{visible_column_widths, Picker};
use view::Editor;

pub(super) type QueryUpdates<D> = Box<dyn FnMut(&mut Editor, &D) -> bool>;

/// Feature-owned state is sampled on redraw without rebuilding unchanged rows.
pub(crate) struct UpdateContext<'a, T> {
    pub editor: &'a mut Editor,
    pub selected: Option<&'a T>,
    pub task: &'a mut event::TaskController,
    /// A cached picker was restored and may have discarded an unpublished refresh.
    pub resumed: bool,
}

type EditorRefresh<T> = Box<dyn FnMut(UpdateContext<'_, T>) -> Option<Vec<T>>>;

pub(super) struct EditorUpdates<T> {
    refresh: EditorRefresh<T>,
    same: fn(&T, &T) -> bool,
    pub task: event::TaskController,
    pub resumed: bool,
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
        self.request_dynamic_refresh(false);
    }

    pub(super) fn request_dynamic_refresh(&mut self, is_paste: bool) {
        let Some(handler) = &self.dynamic_query_handler else {
            return;
        };
        self.dynamic_query_pending = true;
        self.query_refresh = true;
        self.results.discard_pending();

        self.dynamic_query_request.cancel();
        self.results.invalidate();
        event::send_blocking(
            handler,
            super::handlers::DynamicQueryChange {
                query: self.primary_query(),
                is_paste,
            },
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
            resumed: false,
        });
        self
    }

    pub(super) fn refresh_from_editor(&mut self, editor: &mut Editor) {
        if self.results.pending_options.is_some() {
            return;
        }
        if self.results.matcher.tick(0).running {
            return;
        }
        let Some(mut updates) = self.editor_updates.take() else {
            return;
        };
        let resumed = std::mem::take(&mut updates.resumed);
        if let Some(options) = (updates.refresh)(UpdateContext {
            editor,
            selected: self.selection(),
            task: &mut updates.task,
            resumed,
        }) {
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
        self.editor_updates = Some(updates);
    }

    /// Load immutable options under this picker's lifecycle. Hidden/cached pickers
    /// discard pending results and retry on restore; published rows remain stable.
    #[cfg(any(feature = "git", all(test, feature = "integration")))]
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
        let mut pending: Option<tokio::sync::oneshot::Receiver<anyhow::Result<Vec<T>>>> = None;
        let mut complete = false;
        self.with_editor_updates(move |cx| {
            if complete {
                return None;
            }
            if cx.resumed {
                pending = None;
            }
            if let Some(receiver) = &mut pending {
                match receiver.try_recv() {
                    Ok(result) => {
                        pending = None;
                        complete = true;
                        return match result {
                            Ok(options) => Some(options),
                            Err(error) => {
                                cx.editor.set_error(|| error.to_string());
                                None
                            }
                        };
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => pending = None,
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return None,
                }
            }
            let cancel = cx.task.restart();
            let work = load(cancel.clone());
            let redraw = cx.editor.redraw().clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            pending = Some(rx);
            tokio::spawn(async move {
                if let Some(result) = event::cancelable_future(work, cancel).await {
                    let _ = tx.send(result);
                    redraw.request();
                }
            });
            None
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

#[cfg(all(test, feature = "integration"))]
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
