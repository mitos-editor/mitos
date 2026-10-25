use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use editor_core::{
    search::statistics::{self, Statistics},
    Range,
};
use event::{cancelable_future, TaskHandle};
use tokio::sync::Semaphore;

use super::{Query, State};
use crate::{callbacks::EditorCallbackSender, Document, DocumentId, Editor, ViewId};

const DEBOUNCE: Duration = Duration::from_millis(30);
const BUDGET: Duration = Duration::from_millis(40);

/// One editor's callback destination and concurrency limit. Permits stay with
/// blocking work until it actually exits, including after cancellation.
#[derive(Clone)]
pub struct Handler {
    callbacks: EditorCallbackSender,
    workers: Arc<Semaphore>,
}

impl Handler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            callbacks,
            workers: Arc::new(Semaphore::new(2)),
        }
    }

    pub(crate) fn request(
        &self,
        doc: &mut Document,
        view: ViewId,
        query: &Query,
        target: Range,
        limit: Option<usize>,
    ) {
        let Some(request) = Request::prepare(doc, view, query, target, limit) else {
            return;
        };
        let workers = self.workers.clone();
        let callbacks = self.callbacks.clone();
        tokio::spawn(async move {
            let Some(statistics) = request.compute(workers).await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if request.complete(editor, statistics) {
                        editor.redraw().request();
                    }
                })
                .await;
        });
    }
}

struct Request {
    doc: DocumentId,
    view: ViewId,
    query: Query,
    snapshot: buffer::Snapshot,
    limit: Option<usize>,
    cancel: TaskHandle,
}

impl Request {
    fn prepare(
        doc: &mut Document,
        view: ViewId,
        query: &Query,
        target: Range,
        limit: Option<usize>,
    ) -> Option<Self> {
        let current = doc.search_cache_is_current(view);
        let snapshot = doc.snapshot();
        let id = doc.id();
        let state = doc.search_state_mut(view)?;
        if !current
            || !state
                .query
                .as_ref()
                .is_some_and(|old| old.same_pattern(query))
            || state.limit != limit
        {
            *state = State {
                query: Some(query.clone()),
                snapshot: Some(snapshot.clone()),
                limit,
                ..State::default()
            };
        }
        // A shared query scan serves the latest target, even if its callback
        // was already queued before the next navigation command.
        state.target = Some(target);
        if state.statistics.is_some() || state.request.is_running() {
            return None;
        }
        Some(Self {
            doc: id,
            view,
            query: query.clone(),
            snapshot,
            limit,
            cancel: state.request.restart(),
        })
    }

    async fn compute(&self, workers: Arc<Semaphore>) -> Option<Statistics> {
        let permit = cancelable_future(
            async {
                tokio::time::sleep(DEBOUNCE).await;
                workers.acquire_owned().await
            },
            &self.cancel,
        )
        .await?
        .ok()?;
        let cancel = self.cancel.clone();
        let text = self.snapshot.text().clone();
        let regex = self.query.regex().clone();
        let limit = self.limit;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if cancel.is_canceled() {
                return None;
            };
            let deadline = Instant::now() + BUDGET;
            let result = statistics::count_regex(text.slice(..), &regex, limit, || {
                cancel.is_canceled() || Instant::now() >= deadline
            });
            (!cancel.is_canceled()).then_some(result)
        })
        .await
        .ok()?
    }

    fn complete(&self, editor: &mut Editor, statistics: Statistics) -> bool {
        let config = editor.config();
        if self.cancel.is_canceled()
            || !config.statusline.shows_search_position()
            || config.search.match_limit() != self.limit
            || editor
                .tree
                .try_get(self.view)
                .is_none_or(|view| view.document_id() != self.doc)
        {
            return false;
        };
        let Some(doc) = editor.document_mut(self.doc) else {
            return false;
        };
        if !self.snapshot.is_current(&doc.buffer) {
            return false;
        };
        let Some(state) = doc.search_state_mut(self.view) else {
            return false;
        };
        if !state
            .query
            .as_ref()
            .is_some_and(|query| query.same_pattern(&self.query))
        {
            return false;
        };
        state.statistics = Some(Arc::new(statistics));
        true
    }
}
