//! Prepare newly visible buffers and draw terminal frames.

use std::{collections::HashSet, time::Duration};

use tokio::time::Instant;
use view::{graphics::Rect, Document, DocumentId};

use super::App;
use crate::{compositor, job::Callback};

const BACKGROUND_FRAME_DELAY: Duration = Duration::from_millis(33);
const SYNTAX_GRACE: Duration = Duration::from_millis(16);
const CALLBACK_BATCH_SIZE: usize = 64;

#[derive(Default)]
pub(super) struct RenderState {
    /// Documents presented in the previous frame, including unfocused splits.
    visible_documents: HashSet<DocumentId>,
    pub(super) requested: Option<Instant>,
    pub(super) input_sample: Option<(view::performance::Sample, u64)>,
    preparation_us: u64,
    syntax_deadline: Option<Instant>,
    activity_deadline: Option<Instant>,
}

impl<B: tui::backend::Backend + tui::backend::BackendExt> App<B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    pub(super) fn request_frame(&mut self) {
        self.render_state.requested = Some(Instant::now());
    }

    pub(super) fn request_redraw(&mut self) {
        let due = Instant::now() + BACKGROUND_FRAME_DELAY;
        self.render_state.requested =
            Some(self.render_state.requested.map_or(due, |old| old.min(due)));
    }

    /// Wake for delayed user activity even when no input or server events arrive.
    pub(super) fn activity_frame_deadline(&mut self) -> Option<Instant> {
        use crate::ui::{activity_matches, ACTIVITY_DELAY};
        use editor_config::StatusLineElement;

        let config = self.editor.config();
        if !config
            .statusline
            .left
            .contains(&StatusLineElement::Activity)
            && !config
                .statusline
                .right
                .contains(&StatusLineElement::Activity)
        {
            self.render_state.activity_deadline = None;
            return None;
        }
        let now = Instant::now();
        let next = self
            .editor
            .activities()
            .filter(|activity| {
                self.editor.tree.views().any(|(view, focused)| {
                    (!config.statusline.global || focused)
                        && self
                            .editor
                            .document(view.document_id())
                            .is_some_and(|doc| activity_matches(*activity, doc, focused))
                })
            })
            .map(|activity| {
                let visible = activity.started + ACTIVITY_DELAY;
                if visible > now {
                    visible
                } else {
                    now + Duration::from_millis(80)
                }
            })
            .min();
        self.render_state.activity_deadline = next.map(|due| {
            self.render_state
                .activity_deadline
                .map_or(due, |old| old.min(due))
        });
        self.render_state.activity_deadline
    }

    /// The ordinary event pump keeps progressing while a first frame waits for syntax.
    pub(super) fn frame_deadline(&mut self) -> Option<Instant> {
        let due = self.render_state.requested?;
        if due > Instant::now() {
            return Some(due);
        }
        if self.has_new_pending_syntax() {
            Some(
                *self
                    .render_state
                    .syntax_deadline
                    .get_or_insert_with(|| Instant::now() + SYNTAX_GRACE),
            )
        } else {
            self.render_state.syntax_deadline = None;
            Some(due)
        }
    }

    pub(super) fn update_components(&mut self) {
        let started = self
            .render_state
            .input_sample
            .as_ref()
            .map(|_| std::time::Instant::now());
        // Preparation may spawn work that finishes before drawing starts. Clear
        // only requests preceding this update; later completions need another turn.
        self.editor.begin_frame();
        let area = Rect::from(self.terminal.size().expect("Cannot read terminal size"));
        let config = self.config.load();
        let mut cx = compositor::Context {
            config: crate::config::Context {
                current: &config,
                updates: &self.config_updates.0,
            },
            editor: &mut self.editor,
            interaction: &mut self.interaction,
            jobs: &mut self.jobs,
            scroll: None,
            image_picker: self.image_picker.as_ref(),
            is_cursor_owner: false,
        };
        self.compositor.update(area, &mut cx);
        view::features::code_lenses::prepare_visible_code_lenses(cx.editor);
        if let Some(started) = started {
            self.render_state.preparation_us += view::performance::micros(started.elapsed());
        }
    }

    pub(super) fn draw_frame(&mut self) {
        if self.editor.should_close() {
            return;
        }
        let started = self
            .render_state
            .input_sample
            .as_ref()
            .map(|_| std::time::Instant::now());
        self.terminal
            .backend_mut()
            .start_sync()
            .expect("Cannot start synchronized rendering");
        if self.compositor.full_redraw {
            // Fullscreen resize also clears the screen and invalidates the back buffer.
            // Unlike clear(), it does not query the cursor position: that query can
            // block behind the event reader, which filters out cursor reports.
            let area = Rect::from(self.terminal.size().expect("Cannot read terminal size"));
            self.terminal
                .resize(area)
                .expect("Cannot clear the terminal");
            self.compositor.full_redraw = false;
        }

        let config = self.config.load();
        let mut draw = compositor::DrawContext {
            hyperlinks: Default::default(),
            cursor_exclusions: Vec::new(),
            config: &config,
            editor: &self.editor,
            interaction: &self.interaction,
            editor_cursor: None,
            scroll: None,
            is_cursor_owner: false,
        };

        tui::terminal::draw_with_hyperlinks(&mut self.terminal, |buffer| {
            let cursor = self.compositor.render(buffer.area, buffer, &mut draw);
            (cursor, draw.hyperlinks.into_inner())
        })
        .unwrap();

        self.terminal.backend_mut().end_sync().unwrap();
        self.terminal.backend_mut().flush().unwrap();
        let flushed = view::performance::start();
        if let Some(flushed) = flushed
            && let Some((sample, handling_us)) = self.render_state.input_sample.take()
        {
            sample.record_at(
                flushed,
                view::performance::Details {
                    handling_us: Some(handling_us),
                    preparation_us: Some(std::mem::take(&mut self.render_state.preparation_us)),
                    draw_us: started
                        .map(|started| view::performance::micros(flushed.duration_since(started))),
                    ..Default::default()
                },
            );
        }
        if let Some(flushed) = flushed
            && let Some(editor_view) = self.compositor.find::<crate::ui::EditorView>()
            && let Some(completion) = editor_view.completion.as_mut()
            && let Some(sample) = completion.take_painted_sample()
        {
            sample.record_at(flushed, view::performance::Details::default());
        }
        self.render_state.requested = None;
        self.render_state.syntax_deadline = None;
        self.render_state.activity_deadline = None;
        self.render_state.visible_documents.clear();
        self.render_state
            .visible_documents
            .extend(self.editor.tree.views().map(|(view, _)| view.document_id()));
    }

    /// Only newly visible documents may postpone a frame for pending syntax.
    fn has_new_pending_syntax(&self) -> bool {
        self.editor.tree.views().any(|(view, _)| {
            !self
                .render_state
                .visible_documents
                .contains(&view.document_id())
                && self
                    .editor
                    .document(view.document_id())
                    .is_some_and(Document::is_syntax_pending)
        })
    }

    /// Process a bounded batch without waiting for background work to finish.
    pub(super) fn drain_ready_callbacks(&mut self) -> usize {
        let mut handled = 0;
        for _ in 0..CALLBACK_BATCH_SIZE {
            let Ok(callback) = self.jobs.callbacks.try_recv() else {
                break;
            };
            handled += 1;
            self.apply_callback(Ok(Some(callback)));
        }
        handled
    }

    pub(super) fn apply_callback(&mut self, callback: anyhow::Result<Option<Callback>>) {
        let outcome = self.jobs.handle_callback(
            crate::config::Context {
                current: &self.config.load(),
                updates: &self.config_updates.0,
            },
            &mut self.editor,
            &mut self.interaction,
            &mut self.compositor,
            callback,
        );
        match outcome {
            view::callbacks::CallbackOutcome::Redraw => self.request_redraw(),
            view::callbacks::CallbackOutcome::Frame => self.request_frame(),
            view::callbacks::CallbackOutcome::None => {}
        }
    }
}

#[cfg(test)]
mod tests;
