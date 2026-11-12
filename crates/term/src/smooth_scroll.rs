use std::time::Duration;

use editor_core::{
    movement::{Direction, Movement},
    Selection,
};
use view::{
    callbacks::CallbackOutcome,
    document::{Document, Mode},
    editing::movement::{move_lines, LineMotion},
    view::ViewPosition,
    DocumentId, Editor, View, ViewId,
};

use crate::job::{Callback, Jobs};

const FRAME: Duration = Duration::from_millis(8);

type Shown = (ViewPosition, Selection);

type Key = (ViewId, DocumentId);

#[derive(Default)]
pub(crate) struct SmoothScroll {
    pub(crate) task: event::TaskController,
    target: Option<(Key, Shown)>,
    pub(crate) shown: Option<(Key, Shown)>,
}

impl SmoothScroll {
    pub(crate) fn finish(&mut self, editor: &mut Editor) {
        self.task.cancel();

        self.shown = self.target.take().and_then(|(key, target)| {
            let (_, doc) = view_doc(editor, key)?;
            let shown = shown(doc, key.0);
            set_shown(doc, key.0, target);

            Some((key, shown))
        });
    }

    /// what's on screen right now
    fn from(&mut self, editor: &mut Editor, key: Key) -> Shown {
        if self.target.is_some() {
            self.finish(editor);
        }

        match self.shown.take() {
            Some((shown_key, shown)) if shown_key == key => shown,
            _ => shown(view::doc!(editor), key.0),
        }
    }
}

fn view_doc(editor: &mut Editor, (view_id, doc_id): Key) -> Option<(&View, &mut Document)> {
    let view = editor.tree.try_get(view_id)?;
    if view.document_id() != doc_id {
        return None;
    }

    Some((view, editor.documents.get_mut(&doc_id)?))
}

fn shown(doc: &Document, id: ViewId) -> Shown {
    (doc.view_offset(id), doc.selection(id).clone())
}

fn set_shown(doc: &mut Document, id: ViewId, (offset, selection): Shown) {
    doc.set_view_offset(id, offset);
    doc.set_selection(id, selection);
}

fn ease_out(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(2)
}

/// run `scroll`, then glide there
pub(crate) fn animate(editor: &mut Editor, jobs: &mut Jobs, scroll: impl FnOnce(&mut Editor)) {
    if !editor.config().scroll.smooth {
        return scroll(editor);
    }

    let (view, doc) = view::current_ref!(editor);
    let (id, key) = (view.id, (view.id, doc.id()));
    let from = jobs.smooth_scroll.from(editor, key);

    scroll(editor);
    editor.ensure_cursor_in_view(editor.tree.focus());

    let duration = editor.config().scroll.scroll_speed;
    let behaviour = if editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };

    let Some((view, doc)) = view_doc(editor, key) else {
        return;
    };
    let target = shown(doc, id);
    let text = doc.text().slice(..);
    let cursor_at = |selection: &Selection| ViewPosition {
        anchor: selection.primary().cursor(text),
        ..Default::default()
    };

    let Some(view_rows) = view.rows_between(doc, from.0, target.0) else {
        return;
    };
    let cursor_rows = view
        .rows_between(doc, cursor_at(&from.1), cursor_at(&target.1))
        .unwrap_or(0);

    if (view_rows, cursor_rows) == (0, 0) {
        return;
    }

    set_shown(doc, id, from.clone());
    jobs.smooth_scroll.target = Some((key, target.clone()));
    let task = jobs.smooth_scroll.task.restart();
    let sender = jobs.sender();

    tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        let mut frames = tokio::time::interval(FRAME);
        frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last = (0, 0);

        while !task.is_canceled() {
            frames.tick().await;

            let elapsed = start.elapsed() + FRAME;
            let progress = (elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.0);
            let eased = |rows: isize| (rows as f32 * ease_out(progress)).round() as isize;
            let step = (eased(view_rows), eased(cursor_rows));

            let done = progress >= 1.0;
            if step == last && !done {
                continue;
            }

            last = step;
            let (task, from, target) = (task.clone(), from.clone(), target.clone());

            sender
                .send_callback(Callback::ui(move |_, cx| {
                    if task.is_canceled() {
                        return CallbackOutcome::None;
                    }

                    let Some((view, doc)) = view_doc(cx.editor, key) else {
                        return CallbackOutcome::None;
                    };

                    if done {
                        set_shown(doc, id, target);
                        cx.jobs.smooth_scroll.target = None;
                        return CallbackOutcome::Frame;
                    }

                    let (offset, selection) = from;
                    let (view_step, cursor_step) = step;
                    doc.set_view_offset(id, view.offset_by_rows(doc, offset, view_step));
                    doc.set_selection(id, selection);

                    let direction = if cursor_step < 0 {
                        Direction::Backward
                    } else {
                        Direction::Forward
                    };

                    let count = cursor_step.unsigned_abs();
                    move_lines(doc, view, LineMotion::Visual, direction, count, behaviour);

                    CallbackOutcome::Frame
                }))
                .await;

            if done {
                break;
            }
        }
    });
}
