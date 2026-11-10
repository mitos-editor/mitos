//! Demand-driven page loading shared by incremental picker sources.
//!
//! The picker owns published rows. This adapter retains only an in-flight page
//! and, during refresh, the replacement prefix needed for atomic publication.
use super::{updates::Update, UpdateContext};
use futures_util::future::BoxFuture;
use tokio::sync::oneshot;

pub(crate) struct Page<T, C> {
    pub rows: Vec<T>,
    pub next: Option<C>,
}

use super::source::State;

// Estimated retained row and matching-text bytes, not a process RSS limit.
// One bounded page may cross this soft budget before loading pauses.
const INITIAL_BUDGET: usize = 32 * 1024 * 1024;
type Load<T, C> =
    Box<dyn Fn(C, event::TaskHandle) -> BoxFuture<'static, anyhow::Result<Page<T, C>>>>;

pub(crate) struct Paged<T, C> {
    cursor: Option<C>,
    pending: Option<oneshot::Receiver<anyhow::Result<Page<T, C>>>>,
    load: Load<T, C>,
    size: fn(&T) -> usize,
    state: State,
    labels: [&'static str; 3],
    loaded: usize,
    bytes: usize,
    budget: usize,
    replacement: Option<(usize, Vec<T>)>,
    suspend_cursor: Option<fn(&C)>,
}

impl<T: Send + 'static, C: Clone + Send + 'static> Paged<T, C> {
    pub fn new(
        cursor: Option<C>,
        labels: [&'static str; 3],
        size: fn(&T) -> usize,
        load: impl Fn(C, event::TaskHandle) -> BoxFuture<'static, anyhow::Result<Page<T, C>>> + 'static,
    ) -> Self {
        Self {
            state: if cursor.is_some() {
                State::More
            } else {
                State::Complete
            },
            cursor,
            pending: None,
            load: Box::new(load),
            size,
            labels,
            loaded: 0,
            bytes: 0,
            budget: INITIAL_BUDGET,
            replacement: None,
            suspend_cursor: None,
        }
    }

    pub fn with_suspend(mut self, suspend: fn(&C)) -> Self {
        self.suspend_cursor = Some(suspend);
        self
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn reset(&mut self, cursor: Option<C>, task: &mut event::TaskController) {
        task.cancel();
        self.pending = None;
        // Preserve the old visible prefix until its replacement has caught up.
        let target = self
            .replacement
            .as_ref()
            .map_or(self.loaded, |(target, _)| *target);
        self.replacement = Some((target, Vec::new()));
        self.loaded = 0;
        self.bytes = 0;
        self.cursor = cursor;
        self.state = if self.cursor.is_none() {
            State::Complete
        } else if self.state == State::Paused {
            State::Paused
        } else {
            State::More
        };
    }

    pub fn update(&mut self, cx: UpdateContext<'_, T>) -> Option<Update<T>> {
        if cx.resumed {
            self.pending = None;
            if self.state == State::Loading {
                self.state = State::More;
            }
        }
        let mut force = false;
        if cx.toggle_loading && self.cursor.is_some() {
            match self.state {
                State::Paused | State::Failed => {
                    if self.bytes >= self.budget {
                        self.budget = self.bytes.saturating_add(INITIAL_BUDGET);
                    }
                    self.state = State::More;
                    force = true;
                }
                _ => {
                    cx.task.cancel();
                    self.pending = None;
                    self.state = State::Paused;
                }
            }
        }
        let mut update = if self.state == State::Complete {
            self.replacement
                .take()
                .map(|(_, rows)| Update::Replace(rows))
        } else {
            None
        };
        if let Some(pending) = &mut self.pending {
            let result = match pending.try_recv() {
                Ok(result) => Some(result),
                Err(oneshot::error::TryRecvError::Closed) => {
                    Some(Err(anyhow::anyhow!("Picker source stopped")))
                }
                Err(oneshot::error::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.pending = None;
                match result {
                    Ok(page) => {
                        self.loaded += page.rows.len();
                        self.bytes = self
                            .bytes
                            .saturating_add(page.rows.iter().map(self.size).sum::<usize>());
                        self.cursor = page.next;
                        self.state = if self.cursor.is_none() {
                            State::Complete
                        } else if self.bytes >= self.budget {
                            State::Paused
                        } else {
                            State::More
                        };
                        if let Some((target, rows)) = &mut self.replacement {
                            rows.extend(page.rows);
                            if self.loaded >= *target || self.cursor.is_none() {
                                let (_, rows) = self.replacement.take().unwrap();
                                update = Some(Update::Replace(rows));
                            }
                        } else {
                            update = Some(Update::Append(page.rows));
                        }
                    }
                    Err(error) => {
                        self.state = State::Failed;
                        cx.editor.set_error(|| error.to_string());
                    }
                }
            }
        }
        // Publication and matching precede the next demand decision. A staged
        // refresh is not published yet, so it keeps requesting its prefix.
        if update.is_none()
            && self.state == State::More
            && (force
                || self.loaded == 0
                || self.replacement.is_some()
                || !cx.query.trim().is_empty()
                || cx.near_end)
            && let Some(cursor) = self.cursor.clone()
        {
            let cancel = cx.task.restart();
            let work = (self.load)(cursor, cancel.clone());
            let redraw = cx.editor.redraw().clone();
            let (tx, rx) = oneshot::channel();
            self.pending = Some(rx);
            self.state = State::Loading;
            tokio::spawn(async move {
                if let Some(result) = event::cancelable_future(work, cancel).await {
                    let _ = tx.send(result);
                    redraw.request();
                }
            });
        }
        *cx.incomplete = self.state != State::Complete;
        *cx.status = Some(
            match self.state {
                State::More => self.labels[0],
                State::Loading => self.labels[1],
                State::Complete => self.labels[2],
                State::Paused if self.bytes >= self.budget => {
                    "Memory budget reached; continue to load more"
                }
                State::Paused => "Loading paused; continue to load more",
                State::Failed => "Results incomplete; retry loading",
            }
            .into(),
        );
        update
    }
}

impl<T: Send + 'static, C: Clone + Send + 'static> super::PickerSource<T> for Paged<T, C> {
    fn update(&mut self, cx: UpdateContext<'_, T>) -> Option<Update<T>> {
        Paged::update(self, cx)
    }

    fn suspend(&mut self) {
        self.pending = None;
        if self.state == State::Loading {
            self.state = State::More;
        }
        if let (Some(cursor), Some(suspend)) = (&self.cursor, self.suspend_cursor) {
            suspend(cursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::picker::test::application;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn pages() -> Paged<u32, usize> {
        Paged::new(
            Some(0),
            ["More", "Loading", "Complete"],
            |_| 10,
            |offset, _cancel| {
                Box::pin(async move {
                    Ok(Page {
                        rows: (offset as u32..offset as u32 + 2).collect(),
                        next: (offset < 4).then_some(offset + 2),
                    })
                })
            },
        )
    }

    struct Harness {
        app: crate::application::TestApplication,
        task: event::TaskController,
        status: Option<String>,
        incomplete: bool,
    }
    impl Harness {
        fn new() -> Self {
            Self {
                app: application(false).unwrap(),
                task: event::TaskController::new(),
                status: None,
                incomplete: false,
            }
        }
        fn poll(
            &mut self,
            pages: &mut Paged<u32, usize>,
            query: &str,
            toggle: bool,
            resumed: bool,
        ) -> Option<Update<u32>> {
            pages.update(UpdateContext {
                editor: &mut self.app.editor,
                query,
                near_end: false,
                status: &mut self.status,
                incomplete: &mut self.incomplete,
                selected: None,
                task: &mut self.task,
                resumed,
                toggle_loading: toggle,
            })
        }
        async fn next(&mut self, pages: &mut Paged<u32, usize>) -> Update<u32> {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if let Some(update) = self.poll(pages, "needle", false, false) {
                        return update;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap()
        }
    }

    #[tokio::test]
    async fn budget_pauses_without_losing_continuation_and_continue_extends_it() {
        let mut h = Harness::new();
        let mut p = pages();
        p.budget = 20;
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [0, 1]));
        assert_eq!(p.state, State::Paused);
        assert_eq!(p.cursor, Some(2));
        assert!(h.incomplete);
        assert!(h.poll(&mut p, "needle", false, false).is_none());
        assert!(!p.is_pending());
        h.poll(&mut p, "needle", true, false);
        assert!(p.budget > 20);
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [2, 3]));
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [4, 5]));
        assert!(!h.incomplete);
    }

    #[tokio::test]
    async fn canceled_page_is_retried_from_the_same_cursor() {
        let mut h = Harness::new();
        let mut p = pages();
        h.poll(&mut p, "needle", false, false);
        h.task.cancel();
        h.poll(&mut p, "needle", false, true);
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [0, 1]));
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [2, 3]));
    }

    #[tokio::test]
    async fn manual_pause_cancels_pending_work_and_resumes_with_empty_query() {
        let mut h = Harness::new();
        let mut p = pages();
        h.poll(&mut p, "", false, false);
        h.poll(&mut p, "", true, false);
        assert_eq!(p.state, State::Paused);
        assert!(!p.is_pending());
        h.poll(&mut p, "", false, true);
        assert_eq!(p.state, State::Paused);
        h.poll(&mut p, "", true, false);
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [0, 1]));
    }

    #[tokio::test]
    async fn failure_keeps_partial_rows_and_retry_uses_the_failed_cursor() {
        let mut h = Harness::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = attempts.clone();
        let mut p = Paged::new(
            Some(0),
            ["More", "Loading", "Complete"],
            |_| 10,
            move |offset, _cancel| {
                let calls = calls.clone();
                Box::pin(async move {
                    if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                        anyhow::bail!("temporary failure");
                    }
                    Ok(Page {
                        rows: vec![offset as u32],
                        next: None,
                    })
                })
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while p.state != State::Failed {
                h.poll(&mut p, "needle", false, false);
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(h.incomplete);
        assert_eq!(p.cursor, Some(0));
        h.poll(&mut p, "needle", true, false);
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [0]));
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        assert!(!h.incomplete);
    }

    #[tokio::test]
    async fn refresh_replaces_the_visible_prefix_together_then_appends() {
        let mut h = Harness::new();
        let mut p = pages();
        h.next(&mut p).await;
        h.next(&mut p).await;
        p.reset(Some(0), &mut h.task);
        assert!(matches!(h.next(&mut p).await, Update::Replace(rows) if rows == [0, 1, 2, 3]));
        assert!(matches!(h.next(&mut p).await, Update::Append(rows) if rows == [4, 5]));
    }

    #[tokio::test]
    async fn empty_refresh_can_publish_an_empty_replacement() {
        let mut h = Harness::new();
        let mut p = pages();
        h.next(&mut p).await;
        p.reset(None, &mut h.task);
        assert!(
            matches!(h.poll(&mut p, "", false, false), Some(Update::Replace(rows)) if rows.is_empty())
        );
        assert!(!h.incomplete);
    }
}
