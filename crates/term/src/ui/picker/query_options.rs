//! Owner-scoped background matching for hierarchical query sources.

use super::*;
use tokio::sync::oneshot;

type Output<T, D> = anyhow::Result<(Vec<T>, D, usize)>;
pub(super) type AsyncQueryOptions<T, D> = Box<
    dyn Fn(&Editor, &str, &D, Option<&T>, event::TaskHandle) -> BoxFuture<'static, Output<T, D>>,
>;

pub(super) struct PendingQuery<T, D> {
    result: oneshot::Receiver<Output<T, D>>,
    data: Arc<D>,
    preserve_selection: bool,
}

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    /// Nonempty hierarchical queries may scan an entire workspace. Prepare the
    /// owned request here, then do that work outside the application thread.
    pub(in crate::ui) fn with_async_query_options(
        mut self,
        callback: impl Fn(&Editor, &str, &D, Option<&T>, event::TaskHandle) -> BoxFuture<'static, Output<T, D>>
            + 'static,
    ) -> Self {
        self.async_query_options = Some(Box::new(callback));
        self
    }

    pub(super) fn cancel_query_options(&mut self) {
        self.query_task.cancel();
        self.pending_query = None;
        if self.async_query_options.is_some() {
            // A cancelled injector may contain only part of its rows. Never
            // publish that snapshot when an owner resumes or reveals an item.
            self.results.discard_pending();
        }
    }

    pub(super) fn start_query_options(
        &mut self,
        editor: &Editor,
        preserve_selection: bool,
    ) -> bool {
        self.cancel_query_options();
        if self.primary_query().is_empty() {
            return false;
        }
        let Some(source) = &self.async_query_options else {
            return false;
        };
        let cancel = self.query_task.restart();
        if let Some(updates) = &mut self.updates {
            updates.invalidate(super::updates::Invalidation::Query);
        }
        let work = source(
            editor,
            &self.primary_query(),
            &self.results.editor_data,
            self.selection().filter(|_| preserve_selection),
            cancel.clone(),
        );
        let (tx, result) = oneshot::channel();
        self.pending_query = Some(PendingQuery {
            result,
            data: self.results.editor_data.clone(),
            preserve_selection,
        });
        let redraw = self.redraw.clone();
        tokio::spawn(async move {
            if let Some(result) = event::cancelable_future(work, cancel).await {
                let _ = tx.send(result);
                redraw.request();
            }
        });
        self.redraw.request();
        true
    }

    /// Both query sources stage rows with the same selection/publication policy.
    pub(super) fn prepare_query_options(
        &mut self,
        data: Arc<D>,
        cursor: usize,
        preserve_selection: bool,
    ) -> nucleo::Injector<T> {
        let injector =
            self.results
                .prepare_refresh(data, &self.columns, self.file_fn.is_some(), &self.redraw);
        let pending = self.results.pending_options.as_mut().unwrap();
        pending.preserve_selection = preserve_selection;
        pending.cursor = Some(cursor as u32);
        if !preserve_selection {
            self.results.offset = 0;
        }
        self.matching = true;
        injector
    }

    pub(super) fn poll_query_options(&mut self, editor: &Editor) {
        let Some(pending) = &mut self.pending_query else {
            return;
        };
        if !Arc::ptr_eq(&pending.data, &self.results.editor_data) {
            self.cancel_query_options();
            return;
        }
        match pending.result.try_recv() {
            Ok(result) => {
                let preserve = pending.preserve_selection;
                self.pending_query = None;
                match result {
                    Ok((options, data, cursor)) => {
                        let data = Arc::new(data);
                        let injector = self.prepare_query_options(data.clone(), cursor, preserve);
                        let columns = self.columns.clone();
                        let cancel = self.query_task.restart();
                        let redraw = self.redraw.on_drop();
                        tokio::task::spawn_blocking(move || {
                            let _redraw = redraw;
                            for (index, item) in options.into_iter().enumerate() {
                                if index % 256 == 0 && cancel.is_canceled() {
                                    break;
                                }
                                inject_nucleo_item(&injector, &columns, item, &data);
                            }
                        });
                    }
                    Err(error) => editor.status_sender().report_blocking(error),
                }
            }
            Err(oneshot::error::TryRecvError::Closed) => self.pending_query = None,
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn a_superseded_query_cannot_publish_even_if_its_worker_finished() -> anyhow::Result<()> {
        let app = super::super::test::application(false)?;
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        let requests = Arc::new(Mutex::new(std::collections::VecDeque::from([
            first_rx, second_rx,
        ])));
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [Column::new("item", |item: &u32, _: &()| {
                item.to_string().into()
            })],
            0,
            [0],
            (),
            |_, _, _| {},
        )
        .with_query_options(|_, _, _, _| Ok((vec![0], (), 0)))
        .with_async_query_options(move |_, _, _, selected, _| {
            assert!(
                selected.is_none(),
                "unpublished query rows cannot supply a selection"
            );
            let result = requests.lock().unwrap().pop_front().unwrap();
            Box::pin(async move { Ok((result.await?, (), 0)) })
        });
        while picker.results.tick(10).running {
            tokio::task::yield_now().await;
        }
        assert_eq!(picker.selection(), Some(&0));
        picker.prompt.set_line("first".into(), &app.editor);
        picker.handle_prompt_change(false);
        picker.refresh_query_options(&app.editor, false);
        assert!(picker.has_pending_render());
        first_tx.send(vec![1]).unwrap();
        tokio::task::yield_now().await;
        picker.prompt.set_line("second".into(), &app.editor);
        picker.handle_prompt_change(false);
        // A source-index refresh can arrive before the pending query has rows.
        picker.refresh_query_options(&app.editor, true);
        picker.poll_query_options(&app.editor);
        assert!(picker.pending_query.is_some());
        second_tx.send(vec![2]).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while picker.pending_query.is_some() {
                picker.poll_query_options(&app.editor);
                tokio::task::yield_now().await;
            }
        })
        .await?;
        // The pending matcher is populated on its worker; inspect the final rows.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                picker.results.tick(10);
                picker.publish_options();
                if !picker.results.is_refreshing() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(picker.selection(), Some(&2));
        Ok(())
    }

    #[tokio::test]
    async fn suspended_matching_restarts_on_restore() -> anyhow::Result<()> {
        let mut app = super::super::test::application(false)?;
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [Column::new("item", |item: &u32, _: &()| {
                item.to_string().into()
            })],
            0,
            [0],
            (),
            |_, _, _| {},
        )
        .with_query_options(|_, _, _, _| Ok((vec![0], (), 0)))
        .with_async_query_options({
            let requests = requests.clone();
            move |_, _, _, _, _| {
                requests.fetch_add(1, atomic::Ordering::Relaxed);
                Box::pin(std::future::pending())
            }
        });
        picker.prompt.set_line("pending".into(), &app.editor);
        picker.handle_prompt_change(false);
        picker.refresh_query_options(&app.editor, false);
        picker.suspend();
        assert!(picker.pending_query.is_none());
        picker.refresh_query_if_changed(&mut app.editor);
        assert!(picker.pending_query.is_some());
        assert_eq!(requests.load(atomic::Ordering::Relaxed), 2);
        Ok(())
    }

    #[tokio::test]
    async fn clearing_the_query_cancels_background_matching_before_reveal() -> anyhow::Result<()> {
        let app = super::super::test::application(false)?;
        let cancel = Arc::new(Mutex::new(None));
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [Column::new("item", |item: &u32, _: &()| {
                item.to_string().into()
            })],
            0,
            [0],
            (),
            |_, _, _| {},
        )
        .with_query_options(|_, _, _, _| Ok((vec![0], (), 0)))
        .with_async_query_options({
            let cancel = cancel.clone();
            move |_, _, _, _, handle| {
                *cancel.lock().unwrap() = Some(handle);
                Box::pin(std::future::pending())
            }
        });
        picker.prompt.set_line("pending".into(), &app.editor);
        picker.handle_prompt_change(false);
        picker.refresh_query_options(&app.editor, false);
        picker.reveal_query_item(&app.editor, &0);
        assert!(picker.pending_query.is_none());
        assert!(cancel.lock().unwrap().as_ref().unwrap().is_canceled());
        assert_eq!(&*picker.primary_query(), "");
        Ok(())
    }
    #[tokio::test]
    async fn accepting_a_restored_query_before_redraw_restarts_it_without_using_stale_rows(
    ) -> anyhow::Result<()> {
        use super::super::test::{application, until};
        use crate::{config::Config, job::Jobs, keymap::component::PickerAction};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut app = application(false)?;
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(AtomicUsize::new(0));
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [Column::new("item", |item: &u32, _: &()| {
                format!("new query {item}").into()
            })],
            0,
            [0],
            (),
            {
                let accepted = accepted.clone();
                move |_, item, _| accepted.lock().unwrap().push(*item)
            },
        )
        .with_query_options(|_, _, _, _| Ok((vec![0], (), 0)))
        .with_async_query_options({
            let requests = requests.clone();
            move |_, _, _, _, _| {
                requests.fetch_add(1, Ordering::Relaxed);
                Box::pin(std::future::pending())
            }
        });
        until(&mut picker, &mut app, |p| {
            p.selection() == Some(&0) && !p.matching
        })
        .await?;
        picker.prompt.set_line("new query".into(), &app.editor);
        picker.handle_prompt_change(false);
        picker.refresh_query_options(&app.editor, false);
        // Hierarchical matching retains its last published rows while a query runs.
        until(&mut picker, &mut app, |p| {
            p.selection() == Some(&0) && !p.matching
        })
        .await?;
        picker.suspend();
        assert!(picker.query_needs_refresh);
        let pending_before_input = picker.has_pending_render();
        let config = Config::default();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut jobs = Jobs::new();
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
        // Reopening can deliver input before any update/render callback.
        let _ = picker.execute_actions(&[PickerAction::Accept], &mut cx);
        assert!(
            accepted.lock().unwrap().is_empty(),
            "accepted a row from the previous query"
        );
        assert!(
            pending_before_input,
            "a suspended query must keep sidebar actions queued"
        );
        assert!(picker.pending_query.is_some());
        assert_eq!(requests.load(Ordering::Relaxed), 2);
        Ok(())
    }
}
