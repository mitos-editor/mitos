//! Native diff presentation. The renderer accepts arbitrary immutable text reviews.

use std::{sync::Arc, time::Duration};

use event::{TaskController, TaskHandle};
use tokio::sync::{oneshot, Semaphore};
use tui::{
    buffer::Buffer,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use view::{
    features::change_review::{Review, ReviewLineKind, ReviewRequest, ReviewSnapshot},
    graphics::Rect,
};

use crate::{
    compositor::{Component, Context, Event, EventResult},
    ui::panel,
};

mod picker;
pub use picker::ChangeReviewPreview;

/// Shared by the hunk popup, picker pane, and future host diff-view services.
pub struct ReviewView {
    pub title: String,
    pub review: Arc<Review>,
    offset: usize,
    horizontal: usize,
    keys: ui_core::keymap::KeymapState,
    pending: Option<Pending>,
    task: TaskController,
}

impl ReviewView {
    pub const ID: &'static str = "change-review";

    pub fn new(title: String, review: Review) -> Self {
        Self::from_shared(title, Arc::new(review))
    }

    fn from_shared(title: String, review: Arc<Review>) -> Self {
        Self {
            title,
            review,
            offset: 0,
            horizontal: 0,
            keys: ui_core::keymap::KeymapState::default(),
            pending: None,
            task: TaskController::new(),
        }
    }

    fn loading(
        title: String,
        request: ReviewRequest,
        worker: Arc<Semaphore>,
        redraw: event::Redraw,
    ) -> Self {
        let mut view = Self::new(title, Review::notice("Loading diff…"));
        let request = Arc::new(request);
        let (tx, result) = oneshot::channel();
        view.pending = Some(Pending {
            request: request.clone(),
            result,
        });
        let cancel = view.task.restart();
        load(request, None, worker, cancel, tx, redraw);
        view
    }

    fn draw(&self, area: Rect, surface: &mut Buffer, cx: &Context, scroll: usize) {
        if area.is_empty() {
            return;
        }
        let theme = &cx.editor.theme;
        let heading = format!(
            "{}  +{} −{}",
            literal(&self.title),
            self.review.additions,
            self.review.deletions
        );
        surface.set_stringn(
            area.x,
            area.y,
            heading,
            area.width as usize,
            theme.get("ui.text.focus"),
        );
        let number_width = self.review.line_number_width();
        for (row, line) in self
            .review
            .lines
            .iter()
            .skip(scroll)
            .take(area.height.saturating_sub(1) as usize)
            .enumerate()
        {
            let (prefix, style) = match line.kind {
                ReviewLineKind::Context => (" ", theme.get("ui.text")),
                ReviewLineKind::Removed => ("−", theme.get("diff.minus")),
                ReviewLineKind::Added => ("+", theme.get("diff.plus")),
                ReviewLineKind::Header => (" ", theme.get("diff.delta")),
                ReviewLineKind::Notice => (" ", theme.get("ui.text.inactive")),
            };
            let number = |value: Option<usize>| {
                value.map_or_else(
                    || " ".repeat(number_width),
                    |value| format!("{:>number_width$}", value + 1),
                )
            };
            let numbers = if area.width > 25 {
                format!("{} {} ", number(line.before), number(line.after))
            } else {
                String::new()
            };
            let text = Line::from(vec![
                Span::styled(numbers, theme.get("ui.linenr")),
                Span::styled(
                    format!(
                        "{prefix} {}",
                        literal_prefix(&line.text, self.horizontal, area.width as usize)
                    ),
                    style,
                ),
            ]);
            Paragraph::new(text).render(
                Rect::new(area.x, area.y + row as u16 + 1, area.width, 1),
                surface,
            );
        }
    }
}

impl Component for ReviewView {
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        if let Some(pending) = &mut self.pending {
            let result = match pending.result.try_recv() {
                Ok(result) if pending.request.is_current(cx.editor) => {
                    Some(result.map(|snapshot| snapshot.review.clone()))
                }
                Ok(_) => Some(Err(anyhow::anyhow!(
                    "Buffer changed while loading this diff; open it again to refresh"
                ))),
                Err(oneshot::error::TryRecvError::Closed) => {
                    Some(Err(anyhow::anyhow!("Diff preview unavailable")))
                }
                Err(oneshot::error::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.review =
                    result.unwrap_or_else(|error| Arc::new(Review::notice(&error.to_string())));
                self.pending = None;
                cx.editor.redraw().request();
            }
        }
        self.draw(
            panel::content_area(area),
            surface,
            cx,
            cx.scroll.unwrap_or(self.offset),
        );
    }
    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        Some((
            viewport.0,
            (self.review.lines.len() + 1 + panel::VERTICAL_INSET as usize).min(u16::MAX as usize)
                as u16,
        ))
    }
    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending()
    }
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        use crate::keymap::component::DiffAction;
        use ui_core::keymap::Lookup;
        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        match self
            .keys
            .lookup(&cx.config.current.component_keys.diff, *key)
        {
            Lookup::Pending => EventResult::Consumed(None),
            Lookup::Unbound => EventResult::Ignored(None),
            Lookup::Actions(actions) => {
                for action in actions {
                    self.horizontal = match action {
                        DiffAction::ScrollLeft => self.horizontal.saturating_sub(8),
                        DiffAction::ScrollRight => self
                            .horizontal
                            .saturating_add(8)
                            .min(self.review.max_line_chars()),
                    };
                }
                EventResult::Consumed(None)
            }
        }
    }
}

/// Keep controls literal; escape sequences and Markdown in source files are text.
fn literal(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => result.push_str("    "),
            c if c.is_control() => result.push('�'),
            c => result.push(c),
        }
    }
    result
}

fn literal_prefix(text: &str, offset: usize, width: usize) -> String {
    literal(&text.chars().skip(offset).take(width).collect::<String>())
}

struct Pending {
    request: Arc<ReviewRequest>,
    result: oneshot::Receiver<anyhow::Result<Arc<ReviewSnapshot>>>,
}

fn load(
    request: Arc<ReviewRequest>,
    previous: Option<Arc<ReviewSnapshot>>,
    worker: Arc<Semaphore>,
    cancel: TaskHandle,
    tx: oneshot::Sender<anyhow::Result<Arc<ReviewSnapshot>>>,
    redraw: event::Redraw,
) {
    tokio::spawn(async move {
        let work = async {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let Ok(permit) = worker.acquire_owned().await else {
                return;
            };
            let check = cancel.clone();
            let result = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                anyhow::ensure!(!check.is_canceled(), "Diff preview cancelled");
                request.compute_cached(previous)
            })
            .await
            .unwrap_or_else(|error| Err(error.into()));
            if !cancel.is_canceled() {
                let _ = tx.send(result);
                redraw.request();
            }
        };
        event::cancelable_future(work, &cancel).await;
    });
}
