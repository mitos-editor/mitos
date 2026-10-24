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
pub(crate) mod document;

/// Compact presentation for hunk popups and picker previews.
pub struct ReviewView {
    pub title: String,
    pub review: Arc<Review>,
    offset: usize,
    horizontal: usize,
    keys: ui_core::keymap::KeymapState,
    task: TaskController,
    source: Option<Arc<ReviewSource>>,
    engine: Option<DiffEngine>,
    mode_pending: Option<oneshot::Receiver<Arc<Review>>>,
    mode_worker: Arc<Semaphore>,
}

impl ReviewView {
    pub const ID: &'static str = "change-review";

    pub fn new(title: String, review: Review) -> Self {
        Self::from_shared(title, Arc::new(review))
    }

    pub fn from_shared(title: String, review: Arc<Review>) -> Self {
        Self {
            title,
            review,
            offset: 0,
            horizontal: 0,
            keys: ui_core::keymap::KeymapState::default(),
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
        let mut view =
            Self::from_shared(title, snapshot.review.clone()).with_source(snapshot.source.clone());
        view.engine = Some(snapshot.engine());
        view
    }

    fn heading_rows(&self) -> usize {
        1 + usize::from(self.review.fallback.is_some())
    }

    fn toggle_engine(&mut self, redraw: event::Redraw) {
        let current = self
            .engine
            .or_else(|| self.source.as_ref().map(|source| source.engine()));
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

    fn draw(&self, area: Rect, surface: &mut Buffer, cx: &Context, scroll: usize) {
        if area.is_empty() {
            return;
        }
        let theme = &cx.editor.theme;
        let mode = self.review.provider.unwrap_or("line");
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
            let row_area = Rect::new(area.x, area.y + row as u16 + heading_rows, area.width, 1);
            let line_style = line_style(theme, line.kind);
            surface.set_style(row_area, line_style);
            let marker_style = match line.kind {
                ReviewLineKind::Added => theme.get("diff.plus.gutter"),
                ReviewLineKind::Removed => theme.get("diff.minus.gutter"),
                _ => style,
            };
            let number = |value: Option<usize>| {
                value.map_or_else(
                    || " ".repeat(number_width),
                    |value| format!("{:>number_width$}", value + 1),
                )
            };
            let numbers = if area.width as usize > number_width * 2 + 7 {
                format!("{} {} ", number(line.before), number(line.after))
            } else {
                String::new()
            };
            let mut spans = vec![
                Span::styled(numbers, theme.get("ui.linenr")),
                Span::styled(format!("{prefix} "), marker_style),
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
            Paragraph::new(text).render(row_area, surface);
        }
    }
}

#[cfg(test)]
mod tests;

impl ReviewView {
    fn poll(&mut self) {
        if let Some(pending) = &mut self.mode_pending {
            match pending.try_recv() {
                Ok(review) => {
                    self.review = review;
                    self.mode_pending = None;
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.mode_pending = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
    }
}

impl Component for ReviewView {
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        self.poll();
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

/// Theme overrides use `diff.plus.line` / `diff.minus.line`. Existing RGB themes
/// get a subtle tint; terminal palettes keep their own selection background.
pub(crate) fn line_style(theme: &theme::Theme, kind: ReviewLineKind) -> view::graphics::Style {
    use view::graphics::Color;
    let scope = match kind {
        ReviewLineKind::Added => "diff.plus",
        ReviewLineKind::Removed => "diff.minus",
        ReviewLineKind::Header => return theme.get("ui.text.inactive"),
        _ => return theme.get("ui.text"),
    };
    if let Some(style) = theme.try_get_exact(&format!("{scope}.line")) {
        return style;
    }
    let mut style = theme.get("ui.text");
    style.bg = match (theme.get("ui.background").bg, theme.get(scope).fg) {
        (Some(Color::Rgb(r, g, b)), Some(Color::Rgb(dr, dg, db))) => {
            let mix = |a: u8, b: u8| ((u16::from(a) * 7 + u16::from(b)) / 8) as u8;
            Some(Color::Rgb(mix(r, dr), mix(g, dg), mix(b, db)))
        }
        _ => {
            let selection = theme.get("ui.selection");
            selection.bg.or_else(|| {
                selection
                    .add_modifier
                    .contains(view::graphics::Modifier::REVERSED)
                    .then_some(selection.fg)
                    .flatten()
            })
        }
    };
    style
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
            if previous.is_none() {
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            let Ok(permit) = worker.acquire_owned().await else {
                return;
            };
            let check = cancel.clone();
            let result = request.run(previous, check).await;
            drop(permit);
            if !cancel.is_canceled() {
                let _ = tx.send(result);
                redraw.request();
            }
        };
        event::cancelable_future(work, &cancel).await;
    });
}
