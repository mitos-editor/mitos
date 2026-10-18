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
    features::change_review::{
        DiffEngine, Review, ReviewLineKind, ReviewRequest, ReviewSnapshot, ReviewSource,
    },
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
    source: Option<Arc<ReviewSource>>,
    engine: Option<DiffEngine>,
    mode_pending: Option<oneshot::Receiver<Review>>,
    mode_worker: Arc<Semaphore>,
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
            source: None,
            engine: None,
            mode_pending: None,
            mode_worker: Arc::new(Semaphore::new(1)),
        }
    }

    pub fn with_source(mut self, source: Arc<ReviewSource>) -> Self {
        self.source = Some(source);
        self
    }

    fn from_snapshot(title: String, snapshot: &ReviewSnapshot) -> Self {
        Self::from_shared(title, snapshot.review.clone()).with_source(snapshot.source.clone())
    }

    fn heading_rows(&self) -> usize {
        1 + usize::from(self.review.fallback.is_some())
    }

    fn toggle_engine(&mut self, redraw: event::Redraw) {
        let current = self
            .engine
            .or_else(|| self.source.as_ref().map(|source| source.engine()))
            .or_else(|| {
                self.pending
                    .as_ref()
                    .map(|pending| pending.request.engine())
            });
        let Some(current) = current else {
            return;
        };
        self.engine = Some(current.toggled());
        self.recompute_engine(redraw);
    }

    fn recompute_engine(&mut self, redraw: event::Redraw) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let engine = self.engine.unwrap_or(source.engine());
        let cancel = self.task.restart();
        let worker = self.mode_worker.clone();
        let (tx, rx) = oneshot::channel();
        self.mode_pending = Some(rx);
        tokio::spawn(async move {
            let work = async {
                let Ok(permit) = worker.acquire_owned().await else {
                    return;
                };
                let check = cancel.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    source.compute(Some(engine), &|| check.is_canceled())
                })
                .await;
                if !cancel.is_canceled()
                    && let Ok(review) = result
                {
                    let _ = tx.send(review);
                    redraw.request();
                }
            };
            event::cancelable_future(work, &cancel).await;
        });
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
        let mode = if self.pending.is_some() {
            "loading"
        } else {
            self.review.provider.unwrap_or("line")
        };
        let layout = if self.review.provider == Some("syntax")
            && self.review.unchanged_by_provider
            && (self.review.additions != 0 || self.review.deletions != 0)
        {
            " · layout only"
        } else {
            ""
        };
        let pending = if self.mode_pending.is_some() {
            " · loading…"
        } else {
            ""
        };
        let hint = if self.source.is_some() {
            format!(
                " · {} engine",
                crate::keymap::component::action_hint(
                    &cx.config.current.component_keys.diff,
                    &crate::keymap::component::DiffAction::ToggleEngine
                )
            )
        } else {
            String::new()
        };
        let heading = format!(
            "{mode}{layout} · {}  +{} −{}{pending}{hint}",
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
        if let Some(reason) = self.review.fallback
            && area.height > 1
        {
            surface.set_stringn(
                area.x,
                area.y + 1,
                reason.to_string(),
                area.width as usize,
                theme.get("ui.text.inactive"),
            );
        }
        let heading_rows = self.heading_rows() as u16;
        let number_width = self.review.line_number_width();
        for (row, line) in self
            .review
            .lines
            .iter()
            .skip(scroll)
            .take(area.height.saturating_sub(heading_rows) as usize)
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
            let mut spans = vec![
                Span::styled(numbers, theme.get("ui.linenr")),
                Span::styled(format!("{prefix} "), style),
            ];
            spans.extend(source_spans(
                &line.text,
                line.changes.as_deref(),
                self.horizontal,
                area.width as usize,
                if line.changes.is_some() {
                    theme.get("ui.text")
                } else {
                    style
                },
                style.add_modifier(view::graphics::Modifier::BOLD),
            ));
            let text = Line::from(spans);
            Paragraph::new(text).render(
                Rect::new(area.x, area.y + row as u16 + heading_rows, area.width, 1),
                surface,
            );
        }
    }
}

#[cfg(test)]
mod tests;

impl Component for ReviewView {
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        let mut recompute = false;
        if let Some(pending) = &mut self.pending {
            let result = match pending.result.try_recv() {
                Ok(result) if pending.request.is_current(cx.editor) => Some(result),
                Ok(_) => Some(Err(anyhow::anyhow!(
                    "Buffer changed while loading this diff; open it again to refresh"
                ))),
                Err(oneshot::error::TryRecvError::Closed) => {
                    Some(Err(anyhow::anyhow!("Diff preview unavailable")))
                }
                Err(oneshot::error::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                match result {
                    Ok(snapshot) => {
                        self.review = snapshot.review.clone();
                        self.source = Some(snapshot.source.clone());
                        recompute = self
                            .engine
                            .is_some_and(|engine| engine != snapshot.source.engine());
                    }
                    Err(error) => self.review = Arc::new(Review::notice(&error.to_string())),
                }
                self.pending = None;
                cx.editor.redraw().request();
            }
        }
        if recompute {
            self.recompute_engine(cx.editor.redraw().clone());
        }
        if let Some(pending) = &mut self.mode_pending {
            match pending.try_recv() {
                Ok(review) => {
                    self.review = Arc::new(review);
                    self.mode_pending = None;
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.mode_pending = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
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
            (self.review.lines.len() + self.heading_rows() + panel::VERTICAL_INSET as usize)
                .min(u16::MAX as usize) as u16,
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
                    if *action == DiffAction::ToggleEngine {
                        self.toggle_engine(cx.editor.redraw().clone());
                        continue;
                    }
                    self.horizontal = match action {
                        DiffAction::ScrollLeft => self.horizontal.saturating_sub(8),
                        DiffAction::ScrollRight => self
                            .horizontal
                            .saturating_add(8)
                            .min(self.review.max_line_chars()),
                        DiffAction::ToggleEngine => unreachable!(),
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

fn source_spans(
    text: &str,
    ranges: Option<&[std::ops::Range<usize>]>,
    offset: usize,
    width: usize,
    normal: view::graphics::Style,
    changed: view::graphics::Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut content = String::new();
    let mut previous = false;
    let mut range_index = 0;
    for (byte, ch) in text.char_indices().skip(offset).take(width) {
        let emphasized = ranges.is_some_and(|ranges| {
            while range_index < ranges.len() && ranges[range_index].end <= byte {
                range_index += 1;
            }
            ranges
                .get(range_index)
                .is_some_and(|range| range.contains(&byte))
        });
        if emphasized != previous && !content.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut content),
                if previous { changed } else { normal },
            ));
        }
        previous = emphasized;
        match ch {
            '\t' => content.push_str("    "),
            ch if ch.is_control() => content.push('�'),
            ch => content.push(ch),
        }
    }
    if !content.is_empty() {
        spans.push(Span::styled(
            content,
            if previous { changed } else { normal },
        ));
    }
    spans
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
                request.compute_cached_cancellable(previous, &|| check.is_canceled())
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
