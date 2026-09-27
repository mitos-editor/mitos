//! Adapt editor and terminal events to frontend presentation and input.
use super::{Application, TerminalEvent};
use crate::compositor::Event;
use log::warn;
use stdx::path::get_relative_path;
use view::{document::DocumentSavedEventResult, editor::EditorEvent, graphics::Rect};

impl Application {
    pub async fn handle_idle_timeout(&mut self) {
        let config = self.config.load();
        let mut cx = crate::compositor::Context {
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
        let should_render = self.compositor.handle_event(&Event::IdleTimeout, &mut cx);
        if should_render || self.editor.needs_redraw {
            self.render().await;
        }
    }

    pub fn handle_document_write(&mut self, doc_save_event: DocumentSavedEventResult) {
        let doc_save_event = match doc_save_event {
            Ok(event) => event,
            Err(err) => {
                self.jobs.cancel_after_writes();
                self.editor.set_error(|| err.to_string());
                return;
            }
        };

        if self.editor.document(doc_save_event.doc_id).is_none() {
            warn!(
                "received document saved event for non-existent doc id: {}",
                doc_save_event.doc_id
            );
            return;
        }
        self.editor.complete_document_write(&doc_save_event);

        let lines = doc_save_event.text.len_lines(stdx::rope::LINE_TYPE);
        let size = doc_save_event.text.len();

        enum Size {
            Bytes(u16),
            HumanReadable(f32, &'static str),
        }

        impl std::fmt::Display for Size {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Bytes(bytes) => write!(f, "{bytes}B"),
                    Self::HumanReadable(size, suffix) => write!(f, "{size:.1}{suffix}"),
                }
            }
        }

        let size = if size < 1024 {
            Size::Bytes(size as u16)
        } else {
            const SUFFIX: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
            let mut size = size as f32;
            let mut i = 0;
            while i < SUFFIX.len() - 1 && size >= 1024.0 {
                size /= 1024.0;
                i += 1;
            }
            Size::HumanReadable(size, SUFFIX[i])
        };

        // TODO: fix being overwritten by lsp
        self.editor.set_status(format!(
            "'{}' written, {lines}L {size}",
            get_relative_path(&doc_save_event.path).to_string_lossy(),
        ));
    }

    #[inline(always)]
    pub async fn handle_editor_event(&mut self, event: EditorEvent) -> bool {
        log::debug!("received editor event: {:?}", event);

        match event {
            EditorEvent::DocumentSaved(event) => {
                self.handle_document_write(event);
                self.render().await;
            }
            EditorEvent::WorkspaceTrust(request) => {
                crate::handlers::workspace_trust::prompt(
                    &self.editor,
                    &mut self.compositor,
                    request,
                );
                self.render().await;
            }
            EditorEvent::ReloadConfirmation(request) => {
                crate::handlers::auto_reload::prompt_reload_modified(&mut self.compositor, request);
                self.render().await;
            }
            EditorEvent::Completion(update) => {
                crate::handlers::completion::apply_update(
                    &mut self.editor,
                    &mut self.compositor,
                    update,
                );
                self.render().await;
            }
            EditorEvent::Navigation(update) => {
                crate::handlers::navigation::show(&mut self.editor, &mut self.compositor, update);
                self.render().await;
            }
            EditorEvent::Hover(update) => {
                crate::handlers::hover::show(&mut self.editor, &mut self.compositor, update);
                self.render().await;
            }
            EditorEvent::BlameDetails(update) => {
                crate::ui::blame::show(
                    &mut self.editor,
                    &mut self.compositor,
                    update,
                    &self.config.load(),
                );
                self.render().await;
            }
            EditorEvent::SignatureHelp(update) => {
                crate::handlers::signature_help::show_signature_help(
                    &mut self.editor,
                    &mut self.compositor,
                    update,
                );
                self.render().await;
            }
            EditorEvent::ConfigEvent(event) => {
                self.handle_config_events(event);
                self.render().await;
            }
            EditorEvent::LanguageServerMessage((id, call)) => {
                self.handle_language_server_message(call, id).await;
                // limit render calls for fast language server messages
                self.editor.redraw().request();
            }
            EditorEvent::DebuggerEvent((id, payload)) => {
                let needs_render = self.editor.handle_debugger_event(id, payload).await;
                if needs_render {
                    self.render().await;
                }
            }
            EditorEvent::Redraw => {
                self.render().await;
            }
            EditorEvent::IdleTimer => {
                self.editor.clear_idle_timer();
                self.handle_idle_timeout().await;

                #[cfg(feature = "integration")]
                {
                    return true;
                }
            }
        }

        false
    }

    pub async fn handle_terminal_events(&mut self, event: std::io::Result<TerminalEvent>) {
        #[cfg(not(windows))]
        use termina::escape::csi;

        let config = self.config.load();
        let mut cx = crate::compositor::Context {
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
        // Handle key events
        let should_redraw = match event.unwrap() {
            #[cfg(not(windows))]
            termina::Event::WindowResized(termina::WindowSize { rows, cols, .. }) => {
                self.terminal
                    .resize(Rect::new(0, 0, cols, rows))
                    .expect("Unable to resize terminal");

                let area = Rect::from(self.terminal.size().expect("failed to read terminal size"));

                self.compositor.resize(area);

                self.compositor
                    .handle_event(&Event::Resize(cols, rows), &mut cx)
            }
            #[cfg(not(windows))]
            // Ignore keyboard release events.
            termina::Event::Key(termina::event::KeyEvent {
                kind: termina::event::KeyEventKind::Release,
                ..
            }) => false,
            #[cfg(not(windows))]
            termina::Event::Csi(csi::Csi::Mode(csi::Mode::ReportTheme(mode))) => {
                let config = self.config.load();
                let mode = mode.into();
                let mode_changed = self.theme_mode.as_ref().is_none_or(|m| m != &mode);
                if mode_changed
                    && config
                        .theme
                        .as_ref()
                        .is_none_or(|theme| theme.is_adaptive())
                {
                    self.theme_mode = Some(mode);
                    let theme = Self::configured_theme(
                        &self.editor,
                        &config,
                        &self.terminal,
                        self.theme_mode,
                    );
                    let _ = self.editor.set_theme(theme);
                    true
                } else {
                    false
                }
            }
            #[cfg(windows)]
            TerminalEvent::Resize(width, height) => {
                self.terminal
                    .resize(Rect::new(0, 0, width, height))
                    .expect("Unable to resize terminal");

                let area = Rect::from(self.terminal.size().expect("failed to read terminal size"));

                self.compositor.resize(area);

                self.compositor
                    .handle_event(&Event::Resize(width, height), &mut cx)
            }
            #[cfg(windows)]
            // Ignore keyboard release events.
            crossterm::event::Event::Key(crossterm::event::KeyEvent {
                kind: crossterm::event::KeyEventKind::Release,
                ..
            }) => false,
            #[cfg(not(windows))]
            event if event.is_escape() => false,
            event => self.compositor.handle_event(&event.into(), &mut cx),
        };

        if should_redraw && !self.editor.should_close() {
            self.render().await;
        }
    }
}
