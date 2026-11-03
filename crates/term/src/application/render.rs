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
    syntax_deadline: Option<Instant>,
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
    }

    pub(super) fn draw_frame(&mut self) {
        if self.editor.should_close() {
            return;
        }
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
            config: &config,
            editor: &self.editor,
            interaction: &self.interaction,
            editor_cursor: None,
            scroll: None,
            is_cursor_owner: false,
        };

        tui::terminal::draw_with_cursor(&mut self.terminal, |buffer| {
            self.compositor.render(buffer.area, buffer, &mut draw)
        })
        .unwrap();

        self.terminal.backend_mut().end_sync().unwrap();
        self.terminal.backend_mut().flush().unwrap();
        self.render_state.requested = None;
        self.render_state.syntax_deadline = None;
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
        if outcome.needs_redraw() {
            self.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests;
