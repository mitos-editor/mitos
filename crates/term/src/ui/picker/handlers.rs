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

pub(super) struct DynamicQueryHandler<T: 'static + Send + Sync, D: 'static + Send + Sync> {
    callback: Arc<DynQueryCallback<T, D>>,
    jobs: job::JobSender,
    picker_version: Arc<atomic::AtomicUsize>,
    // Duration used as a debounce.
    // Defaults to 100ms if not provided via `Picker::with_dynamic_query`. Callers may want to set
    // this higher if the dynamic query is expensive - for example global search.
    debounce: Duration,
    last_query: Arc<str>,
    last_query_version: usize,
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
            last_query: "".into(),
            last_query_version: 0,
            query: None,
        }
    }
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> AsyncHook for DynamicQueryHandler<T, D> {
    type Event = DynamicQueryChange;

    fn handle_event(&mut self, change: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let DynamicQueryChange { query, is_paste } = change;
        if query == self.last_query
            && self.picker_version.load(atomic::Ordering::Relaxed) == self.last_query_version
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
        self.last_query = query.clone();
        let callback = self.callback.clone();
        let picker_version = self.picker_version.clone();
        // Supersede older queued queries and their injectors before publication.
        let version = picker_version.fetch_add(1, atomic::Ordering::Relaxed) + 1;
        self.last_query_version = version;

        self.jobs.send_blocking(move |editor, compositor| {
            let Some(Overlay {
                content: picker, ..
            }) = compositor.find::<Overlay<Picker<T, D>>>()
            else {
                return;
            };
            if !Arc::ptr_eq(&picker.version, &picker_version)
                || picker.version.load(atomic::Ordering::Relaxed) != version
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
            picker.matcher.restart(false);
            let injector = picker.injector();
            let get_options = (callback)(&query, editor, picker.editor_data.clone(), &injector);
            tokio::spawn(async move {
                if let Err(err) = get_options.await {
                    log::info!("Dynamic request failed: {err}");
                }
                // NOTE: the Drop implementation of Injector will request a redraw when the
                // injector falls out of scope here, clearing the "running" indicator.
            });
        })
    }
}

#[cfg(all(test, feature = "integration"))]
#[path = "dynamic_query_tests.rs"]
mod tests;
