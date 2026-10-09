//! Prepare newly visible buffers and draw terminal frames.

use std::{collections::HashSet, time::Duration};

use tokio::time::{timeout_at, Instant};
use tui::backend::{Backend, BackendExt};
use view::{graphics::Rect, Document, DocumentId};

use super::Application;
use crate::{compositor, job::Callback};

const SYNTAX_GRACE: Duration = Duration::from_millis(16);
const CALLBACK_BATCH_SIZE: usize = 64;

#[derive(Default)]
pub(super) struct RenderState {
    /// Documents presented in the previous frame, including unfocused splits.
    visible_documents: HashSet<DocumentId>,
}

impl Application {
    pub(super) async fn render(&mut self) {
        if self.has_new_pending_syntax() {
            self.drain_ready_callbacks();
            self.wait_for_syntax().await;
        }
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
        let mut cx = compositor::Context {
            config: crate::config::Context {
                current: &config,
                updates: &self.config_updates.0,
            },
            editor: &mut self.editor,
            jobs: &mut self.jobs,
            scroll: None,
            image_picker: self.image_picker.as_ref(),
            is_cursor_owner: false,
        };

        event::start_frame();
        cx.editor.needs_redraw = false;

        tui::terminal::draw_with_cursor(&mut self.terminal, |buffer| {
            self.compositor.render(buffer.area, buffer, &mut cx)
        })
        .unwrap();
        self.editor.cursor_cache.reset();

        self.terminal.backend_mut().end_sync().unwrap();
        self.terminal.backend_mut().flush().unwrap();
        self.render_state.visible_documents.clear();
        self.render_state
            .visible_documents
            .extend(self.editor.tree.views().map(|(view, _)| view.doc));
    }

    /// Only initial presentation may pull syntax completions ahead of normal events.
    fn has_new_pending_syntax(&self) -> bool {
        self.editor.tree.views().any(|(view, _)| {
            !self.render_state.visible_documents.contains(&view.doc)
                && self
                    .editor
                    .document(view.doc)
                    .is_some_and(Document::is_syntax_pending)
        })
    }

    /// Give newly visible buffers one shared deadline before presenting their first frame.
    async fn wait_for_syntax(&mut self) {
        let deadline = Instant::now() + SYNTAX_GRACE;
        for _ in 0..CALLBACK_BATCH_SIZE {
            if self.editor.should_close()
                || !self.has_new_pending_syntax()
                || Instant::now() >= deadline
            {
                break;
            }
            let Ok(Some(callback)) = timeout_at(deadline, self.jobs.callbacks.recv()).await else {
                break;
            };
            self.apply_callback(callback);
        }
    }

    /// Process a bounded batch without waiting for background work to finish.
    pub(super) fn drain_ready_callbacks(&mut self) -> bool {
        let mut handled_callbacks = false;
        for _ in 0..CALLBACK_BATCH_SIZE {
            let Ok(callback) = self.jobs.callbacks.try_recv() else {
                break;
            };
            self.apply_callback(callback);
            handled_callbacks = true;
        }
        handled_callbacks
    }

    fn apply_callback(&mut self, callback: Callback) {
        if let Some(job) =
            self.jobs
                .handle_callback(&mut self.editor, &mut self.compositor, Ok(Some(callback)))
        {
            self.jobs.add(job);
        }
    }
}

#[cfg(all(test, feature = "integration"))]
mod tests;
