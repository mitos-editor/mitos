//! Selected historical sources use the same diff renderer and native document opening.
use super::ReviewView;
use crate::{
    compositor::{Context, EventResult},
    ui::picker::{PickerPreview, PreviewAction},
};
use event::TaskController;
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;
use tui::buffer::Buffer;
use view::{
    features::change_review::{HistoricalReviewRequest, HistoricalReviewSnapshot},
    graphics::Rect,
    Editor,
};

type Capture<T> = Box<dyn Fn(&Editor, &T) -> anyhow::Result<HistoricalReviewRequest>>;
type Key<T, K> = Box<dyn Fn(&T) -> Option<K>>;

/// One selected comparison, canceled when hidden or replaced; pagination rows have no key.
pub struct HistoricalReviewPreview<T, K> {
    key: Key<T, K>,
    capture: Capture<T>,
    selected: Option<(K, HistoricalReviewRequest)>,
    task: TaskController,
    pending: Option<oneshot::Receiver<anyhow::Result<Arc<HistoricalReviewSnapshot>>>>,
    snapshot: Option<Arc<HistoricalReviewSnapshot>>,
    ready: Option<ReviewView>,
    error: Option<String>,
    height: u16,
    opened_document: bool,
}

impl<T, K: Eq> HistoricalReviewPreview<T, K> {
    pub fn new(
        key: impl Fn(&T) -> Option<K> + 'static,
        capture: impl Fn(&Editor, &T) -> anyhow::Result<HistoricalReviewRequest> + 'static,
    ) -> Self {
        Self {
            key: Box::new(key),
            capture: Box::new(capture),
            selected: None,
            task: TaskController::new(),
            pending: None,
            snapshot: None,
            ready: None,
            error: None,
            height: 1,
            opened_document: false,
        }
    }

    fn stop(&mut self) {
        self.task.cancel();
        self.pending = None;
        self.selected = None;
        self.snapshot = None;
        self.ready = None;
        self.error = None;
    }

    fn select(&mut self, item: Option<&T>, editor: &Editor) {
        let Some((item, key)) = item.and_then(|item| (self.key)(item).map(|key| (item, key)))
        else {
            self.stop();
            return;
        };
        let request = match (self.capture)(editor, item) {
            Ok(request) => request,
            Err(error) => {
                self.stop();
                self.error = Some(error.to_string());
                return;
            }
        };
        if self
            .selected
            .as_ref()
            .is_none_or(|(old, policy)| *old != key || !policy.same_policy(&request))
        {
            self.stop();
            self.selected = Some((key, request));
        }
    }
}

impl<T, K: Eq> PickerPreview<T> for HistoricalReviewPreview<T, K> {
    fn cancel(&mut self) {
        self.stop();
    }
    fn open_on_accept(&self, item: &T) -> bool {
        (self.key)(item).is_some()
    }
    fn close_on_open(&self) -> bool {
        self.opened_document
    }

    fn render(&mut self, item: Option<&T>, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        self.select(item, cx.editor);
        self.height = area.height;
        if let Some((_, request)) = &self.selected
            && self.pending.is_none()
            && self.snapshot.is_none()
            && self.error.is_none()
        {
            let request = request.clone();
            let cancel = self.task.restart();
            let redraw = cx.editor.redraw().clone();
            let (tx, rx) = oneshot::channel();
            self.pending = Some(rx);
            tokio::spawn(async move {
                let work = async {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    let result = request.run(cancel.clone()).await;
                    if !cancel.is_canceled() {
                        let _ = tx.send(result);
                        redraw.request();
                    }
                };
                event::cancelable_future(work, &cancel).await;
            });
        }
        if let Some(pending) = &mut self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(snapshot) => {
                            let view = ReviewView::from_shared(
                                "Historical change".into(),
                                snapshot.review.clone(),
                            );
                            self.ready = Some(view);
                            self.snapshot = Some(snapshot);
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.pending = None;
                    self.error = Some("Historical preview unavailable".into());
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        let content = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        if let Some(view) = &mut self.ready {
            view.offset =
                view.offset.min(view.review.lines.len().saturating_sub(
                    content.height.saturating_sub(view.heading_rows() as u16) as usize,
                ));
            view.draw(content, surface, cx, view.offset);
        } else if !content.is_empty() {
            let message = self.error.as_deref().unwrap_or(if self.selected.is_some() {
                "Loading diff…"
            } else {
                "Select a commit or file to preview its diff"
            });
            surface.set_stringn(
                content.x,
                content.y,
                message,
                content.width as usize,
                cx.editor.theme.get("ui.text.inactive"),
            );
        }
        if area.height > 1 {
            use crate::keymap::component::{action_hint, PickerAction};
            let keys = &cx.config.current.component_keys.history_picker;
            surface.set_stringn(
                area.x,
                area.bottom() - 1,
                format!(
                    "{} open · {}/{} scroll",
                    action_hint(keys, &PickerAction::OpenPreview),
                    action_hint(keys, &PickerAction::PreviewScrollUp),
                    action_hint(keys, &PickerAction::PreviewScrollDown)
                ),
                area.width as usize,
                cx.editor.theme.get("ui.text.inactive"),
            );
        }
    }

    fn action(&mut self, item: Option<&T>, action: PreviewAction, cx: &mut Context) -> EventResult {
        self.opened_document = false;
        self.select(item, cx.editor);
        match action {
            PreviewAction::Open(action) => {
                if let Some((_, request)) = &self.selected {
                    if let Some(snapshot) = &self.snapshot {
                        match cx.editor.open_historical_snapshot(snapshot.clone(), action) {
                            Ok(_) => self.opened_document = true,
                            Err(error) => cx.editor.set_error(|| error.to_string()),
                        }
                    } else {
                        cx.editor.open_historical_review(request.clone(), action);
                        self.opened_document = true;
                    }
                }
            }
            PreviewAction::ScrollUp => {
                if let Some(view) = &mut self.ready {
                    view.offset = view
                        .offset
                        .saturating_sub((self.height / 2).max(1) as usize);
                }
            }
            PreviewAction::ScrollDown => {
                if let Some(view) = &mut self.ready {
                    view.offset = view
                        .offset
                        .saturating_add((self.height / 2).max(1) as usize);
                }
            }
            PreviewAction::Previous | PreviewAction::Next => {}
        }
        EventResult::Consumed(None)
    }
}

#[cfg(all(test, feature = "integration"))]
mod tests {
    use super::*;
    use crate::{application::Application, job::Jobs, ui::picker::test::application};

    fn render(
        preview: &mut HistoricalReviewPreview<u8, u8>,
        selected: u8,
        app: &mut Application,
    ) -> Buffer {
        let config = crate::config::Config::default();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut jobs = Jobs::new();
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
        let area = Rect::new(0, 0, 80, 20);
        let mut surface = Buffer::empty(area);
        preview.render(Some(&selected), area, &mut surface, &mut cx);
        surface
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn selection_changes_and_hidden_previews_discard_pending_historical_sources(
    ) -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("history.txt");
        let workspace = dir.path().to_owned();
        let mut preview = HistoricalReviewPreview::new(
            |item: &u8| (*item != 0).then_some(*item),
            move |editor, item| {
                Ok(HistoricalReviewRequest::comparison(
                    editor,
                    path.clone(),
                    workspace.clone(),
                    format!("revision {item}"),
                    vcs::Comparison {
                        before: format!("removed {item}\n").into_bytes(),
                        after: format!("added {item}\n").into_bytes(),
                    },
                ))
            },
        );
        render(&mut preview, 1, &mut app);
        assert!(preview.pending.is_some());
        render(&mut preview, 2, &mut app);
        tokio::time::timeout(Duration::from_secs(5), async {
            while preview.snapshot.is_none() {
                render(&mut preview, 2, &mut app);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let surface = render(&mut preview, 2, &mut app);
        let text: String = surface.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("removed 2") && text.contains("added 2"));
        assert!(!text.contains("removed 1"));
        assert_eq!(
            preview
                .snapshot
                .as_ref()
                .unwrap()
                .source
                .after()
                .to_string(),
            "added 2\n"
        );
        preview.cancel();
        assert!(preview.snapshot.is_none() && preview.pending.is_none());
        render(&mut preview, 1, &mut app);
        assert!(preview.pending.is_some());
        let surface = render(&mut preview, 0, &mut app);
        assert!(preview.pending.is_none() && preview.snapshot.is_none());
        assert!(
            !preview.open_on_accept(&0),
            "pagination rows must use normal picker acceptance"
        );
        let text: String = surface.content.iter().map(|cell| cell.symbol()).collect();
        assert!(!text.contains("removed"));
        Ok(())
    }
}
