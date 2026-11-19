use std::{
    sync::{atomic, Arc},
    time::Duration,
};

use event::AsyncHook;
use tokio::time::Instant;

use crate::{job, ui::overlay::Overlay};

use super::{DynQueryCallback, Picker};

pub(super) struct DynamicQueryChange {
    pub query: Arc<str>,
    pub is_paste: bool,
}

/// Whether queued work replaces a query or refreshes its existing rows.
#[derive(Clone, Copy)]
pub(super) enum QueryKind {
    Replace,
    Refresh,
}

/// One dynamic source owns its debounce channel, pending intent, and running request.
pub(super) struct DynamicQuery {
    handler: tokio::sync::mpsc::Sender<DynamicQueryChange>,
    request: event::TaskController,
    pending: Option<QueryKind>,
}

impl DynamicQuery {
    pub fn new(handler: tokio::sync::mpsc::Sender<DynamicQueryChange>, query: Arc<str>) -> Self {
        // Treat the initial query as a paste, without invalidating existing injectors.
        event::send_blocking(
            &handler,
            DynamicQueryChange {
                query,
                is_paste: true,
            },
        );
        Self {
            handler,
            request: event::TaskController::new(),
            pending: Some(QueryKind::Replace),
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn suspend(&mut self) {
        self.pending = None;
        self.request.cancel();
    }

    pub fn schedule(
        &mut self,
        change: DynamicQueryChange,
        kind: Option<QueryKind>,
        version: &atomic::AtomicUsize,
    ) {
        if let Some(kind) = kind {
            self.pending = Some(kind);
            self.request.cancel();
            version.fetch_add(1, atomic::Ordering::Relaxed);
        }
        event::send_blocking(&self.handler, change);
    }
}

pub(super) struct DynamicQueryHandler<T: 'static + Send + Sync, D: 'static + Send + Sync> {
    callback: Arc<DynQueryCallback<T, D>>,
    jobs: job::JobSender,
    picker_version: Arc<atomic::AtomicUsize>,
    // Duration used as a debounce.
    // Defaults to 100ms if not provided via `Picker::with_dynamic_query`. Callers may want to set
    // this higher if the dynamic query is expensive - for example global search.
    debounce: Duration,
    last_query: Option<(Arc<str>, usize)>,
    query: Option<Arc<str>>,
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> DynamicQueryHandler<T, D> {
    pub(super) fn new(
        callback: DynQueryCallback<T, D>,
        duration_ms: Option<u64>,
        jobs: job::JobSender,
        picker_version: Arc<atomic::AtomicUsize>,
    ) -> Self {
        Self {
            callback: Arc::new(callback),
            jobs,
            picker_version,
            debounce: Duration::from_millis(duration_ms.unwrap_or(100)),
            last_query: None,
            query: None,
        }
    }
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> AsyncHook for DynamicQueryHandler<T, D> {
    type Event = DynamicQueryChange;

    fn handle_event(&mut self, change: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let DynamicQueryChange { query, is_paste } = change;
        if self
            .last_query
            .as_ref()
            .is_some_and(|(last_query, version)| {
                query == *last_query
                    && self.picker_version.load(atomic::Ordering::Relaxed) == *version
            })
        {
            // If the search query reverts to the last one we requested, no need to
            // make a new request.
            self.query = None;
            None
        } else {
            self.query = Some(query);
            if is_paste {
                self.finish_debounce();
                None
            } else {
                Some(Instant::now() + self.debounce)
            }
        }
    }

    fn finish_debounce(&mut self) {
        let Some(query) = self.query.take() else {
            return;
        };
        let callback = self.callback.clone();
        let picker_version = self.picker_version.clone();
        // Supersede older queued queries and their injectors before publication.
        let version = picker_version.fetch_add(1, atomic::Ordering::Relaxed) + 1;
        self.last_query = Some((query.clone(), version));

        self.jobs.send_blocking(move |compositor, cx| {
            let editor = &mut *cx.editor;
            let Some(Overlay {
                content: picker, ..
            }) = compositor.find::<Overlay<Picker<T, D>>>()
            else {
                return;
            };
            if !Arc::ptr_eq(&picker.results.version, &picker_version)
                || picker.results.version.load(atomic::Ordering::Relaxed) != version
            {
                return;
            }
            if picker.primary_query() != query {
                // A query skipped during debounce must remain eligible when the
                // user returns to it. Preserve any newer queued generation.
                let _ = picker_version.compare_exchange(
                    version,
                    version + 1,
                    atomic::Ordering::Relaxed,
                    atomic::Ordering::Relaxed,
                );
                return;
            }
            let Some(dynamic_query) = &mut picker.dynamic_query else {
                return;
            };
            let refresh = matches!(dynamic_query.pending.take(), Some(QueryKind::Refresh));
            let cancel = dynamic_query.request.restart();
            let injector = if refresh {
                picker.refresh_injector()
            } else {
                picker.results.discard_pending();

                picker.results.preserve_selection = false;
                picker.results.matcher.restart(false);
                picker.injector()
            };
            let get_options = (callback)(
                &query,
                editor,
                picker.results.editor_data.clone(),
                &injector,
            );
            tokio::spawn(async move {
                match event::cancelable_future(get_options, &cancel).await {
                    Some(Ok(())) => injector.complete(),
                    Some(Err(err)) => {
                        injector.fail();
                        log::info!("Dynamic request failed: {err}");
                    }
                    None => injector.pause(),
                }
                // NOTE: the Drop implementation of Injector will request a redraw when the
                // injector falls out of scope here, clearing the "running" indicator.
            });
        })
    }
}

#[cfg(test)]
#[path = "dynamic_query_tests.rs"]
mod tests;
