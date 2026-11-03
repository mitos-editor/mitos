//! Frontend state, event loop, signals, and shutdown.
//! Private adapters own startup, configuration, protocol dispatch, and rendering.

use arc_swap::ArcSwap;
use futures_util::Stream;
use tui::backend::{Backend, BackendExt};
use view::Editor;

mod configuration;
mod events;
mod input;
mod interactive;
mod lsp;
mod render;
#[cfg(not(windows))]
mod signals;
mod startup;

use crate::{compositor::Compositor, config::Config, job::Jobs};

use std::sync::Arc;

use anyhow::Error;

#[cfg(not(windows))]
type Signal = i32;
#[cfg(windows)]
type Signal = ();
type Signals = futures_util::stream::BoxStream<'static, Signal>;

#[cfg(not(windows))]
use tui::backend::TerminaBackend;

#[cfg(windows)]
use tui::backend::CrosstermBackend;

use tui::backend::TestBackend;

#[cfg(not(windows))]
type TerminalBackend = TerminaBackend;
#[cfg(windows)]
type TerminalBackend = CrosstermBackend<std::io::Stdout>;

#[cfg(not(windows))]
type TerminalEvent = termina::Event;
#[cfg(windows)]
type TerminalEvent = crossterm::event::Event;

pub type Application = App<TerminalBackend>;
pub type TestApplication = App<TestBackend>;

pub struct App<B: Backend + BackendExt> {
    compositor: Compositor,
    terminal: tui::Terminal<B>,
    render_state: render::RenderState,
    pub editor: Editor,
    image_picker: Option<ratatui_image::picker::Picker>,

    config: Arc<ArcSwap<Config>>,
    config_updates: (
        tokio::sync::mpsc::UnboundedSender<crate::config::ConfigEvent>,
        tokio::sync::mpsc::UnboundedReceiver<crate::config::ConfigEvent>,
    ),

    signals: Signals,
    jobs: Jobs,
    terminal_input: Option<input::Control>,
    _presentation_hooks: event::HookRegistration,

    theme_mode: Option<theme::Mode>,
}

impl<B: Backend + BackendExt> App<B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    /// Screen and cursor state exposed to the integration test harness.
    pub fn terminal_backend(&self) -> &B {
        self.terminal.backend()
    }

    /// Queue a completion for this application's editor and compositor.
    pub fn job_sender(&self) -> crate::job::JobSender {
        self.jobs.sender()
    }

    pub async fn event_loop<S>(&mut self, input_stream: &mut S)
    where
        S: Stream<Item = std::io::Result<TerminalEvent>> + Unpin,
    {
        self.render().await;

        loop {
            if !self.pump(input_stream, false).await {
                break;
            }
        }
    }

    pub async fn event_loop_until_idle<S>(&mut self, input_stream: &mut S) -> bool
    where
        S: Stream<Item = std::io::Result<TerminalEvent>> + Unpin,
    {
        self.pump(input_stream, true).await
    }

    async fn pump<S>(&mut self, input_stream: &mut S, until_idle: bool) -> bool
    where
        S: Stream<Item = std::io::Result<TerminalEvent>> + Unpin,
    {
        loop {
            self.jobs.run_after_writes(&mut self.editor);
            if self.editor.should_close() {
                return false;
            }

            // One input event can enqueue several callbacks. Drain a bounded
            // batch before accepting more input so synchronous event hooks do
            // not spend every keystroke waiting for space in their own queue.
            let batch = self.drain_ready_callbacks();
            if batch.handled > 0 {
                if batch.needs_redraw {
                    self.render().await;
                }
                if until_idle {
                    self.editor.reset_idle_timer();
                }
                // Callbacks can close the editor as well as change its state.
                if self.editor.should_close() {
                    return false;
                }
            }

            if self.editor.should_close() {
                return false;
            }

            use futures_util::StreamExt;

            // Keep callbacks and saves progressing even when terminal input is
            // continuously ready; input priority can fill the callback queue.
            tokio::select! {
                Some(signal) = self.signals.next() => {
                    if !self.handle_signals(signal).await {
                        return false;
                    };
                }
                Some(request) = self.jobs.interactive_requests.recv() => {
                    match self.interactive_shell(request).await {
                        Ok(true) => {},
                        Ok(false) => return false,
                        Err(error) => {
                            log::error!("Could not restore terminal after interactive shell: {error}");
                            return false;
                        }
                    }
                }
                Some(event) = input_stream.next() => {
                    self.handle_terminal_events(event).await;
                }
                Some(event) = self.config_updates.1.recv() => {
                    self.apply_config_update(event);
                    self.render().await;
                }
                Some(callback) = self.jobs.callbacks.recv() => {
                    let outcome = self.jobs.handle_callback(crate::config::Context { current: &self.config.load(), updates: &self.config_updates.0 }, &mut self.editor, &mut self.compositor, Ok(Some(callback)));
                    if outcome.needs_redraw() { self.render().await; }
                }
                Some(callback) = self.jobs.wait_futures.next() => {
                    let outcome = self.jobs.handle_callback(crate::config::Context { current: &self.config.load(), updates: &self.config_updates.0 }, &mut self.editor, &mut self.compositor, callback);
                    if outcome.needs_redraw() { self.render().await; }
                }
                event = self.editor.wait_event() => {
                    let idle_handled = self.handle_editor_event(event).await;

                    if until_idle {
                        // Don't report idle while a save is still in flight, or an assertion on the post-save document state (e.g. its path,
                        // set in `handle_document_write`) can run before the `DocumentSavedEvent` is processed. Slow file I/O on Windows
                        // (atomic_save's rename/fsync dance over the still-open temp file) makes this race observable.
                        // Initial syntax must also settle before assertions; its completion can
                        // schedule spelling checks. Selection shell results must also settle before
                        // command assertions inspect their edits. Completions arrive via jobs.
                        if idle_handled
                            && self.jobs.wait_futures.is_empty()
                            && !self.editor.has_pending_writes()
                            && !self.editor.has_pending_shell_filters()
                            && !self.jobs.command_expansion.is_running()
                            && !self.editor.documents().any(|doc| doc.is_syntax_pending())
                            && !self.compositor.has_pending_render()
                        {
                            use futures_util::FutureExt;

                            // An expired idle timer can win the select while input is
                            // still queued, especially when a CI runner is busy.
                            if let Some(Some(event)) = input_stream.next().now_or_never() {
                                self.handle_terminal_events(event).await;
                            } else {
                                return true;
                            }
                        }
                    }
                }
            }

            // The explicit idle pump waits for accepted work and queued input.
            if until_idle {
                self.editor.reset_idle_timer();
            }
        }
    }

    #[cfg(windows)]
    // no signal handling available on windows
    pub async fn handle_signals(&mut self, _signal: ()) -> bool {
        true
    }

    fn restore_term(&mut self) -> std::io::Result<()> {
        self.terminal.show_cursor().ok();
        self.terminal.backend_mut().restore()
    }

    pub async fn run<S>(&mut self, input_stream: &mut S) -> Result<i32, Error>
    where
        S: Stream<Item = std::io::Result<TerminalEvent>> + Unpin,
    {
        self.terminal.backend_mut().claim()?;

        if self.terminal_input.is_some() {
            self.image_picker = ratatui_image::picker::Picker::from_query_stdio()
                .map_err(|err| {
                    log::warn!(
                        "failed to detect terminal image capabilities, disabling image previews: {err}"
                    );
                })
                .ok()
                .filter(|picker| {
                    picker.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks
                });
            log::debug!(
                "terminal image protocol: {:?}",
                self.image_picker
                    .as_ref()
                    .map(|picker| picker.protocol_type())
            );
        }

        self.event_loop(input_stream).await;

        let close_errs = self.close().await;

        self.restore_term()?;

        for err in close_errs {
            self.editor.exit_code = 1;
            eprintln!("Error: {}", err);
        }

        Ok(self.editor.exit_code)
    }

    pub async fn close(&mut self) -> Vec<anyhow::Error> {
        // [NOTE] we intentionally do not return early for errors because we
        //        want to try to run as much cleanup as we can, regardless of
        //        errors along the way
        let mut errs = Vec::new();

        // Continue serving formatter/code-action callbacks while all accepted saves drain.
        // A failed operation must not abandon unrelated accepted jobs or writes.
        self.jobs.cancel_after_writes();
        while !self.jobs.wait_futures.is_empty() || self.editor.has_pending_writes() {
            use futures_util::StreamExt;
            use view::editor::EditorEvent;
            tokio::select! {
                result = async {
                    tokio::select! {
                        Some(result) = self.jobs.wait_futures.next() => result,
                        Some(callback) = self.jobs.callbacks.recv() => Ok(Some(callback)),
                    }
                } => {
                    match self.jobs.try_handle_callback(
                        crate::config::Context { current: &self.config.load(), updates: &self.config_updates.0 },
                        &mut self.editor, &mut self.compositor, result,
                    ) {
                        Ok(()) => (),
                        Err(error) => errs.push(error),
                    }
                }
                event = self.editor.wait_event() => {
                    if let EditorEvent::DocumentSaved(Err(error)) = event {
                        errs.push(error);
                    } else {
                        self.handle_editor_event(event).await;
                    }
                }
            }
        }

        self.editor.close_language_servers(None).await;

        errs
    }
}

#[cfg(test)]
mod save_tests;

impl Application {
    #[cfg(not(windows))]
    pub fn event_stream(
        &self,
    ) -> impl Stream<Item = std::io::Result<TerminalEvent>> + Unpin + use<> {
        use termina::Terminal as _;
        input::Events::new(
            self.terminal.backend().terminal().event_reader(),
            self.terminal_input.clone().expect("native terminal input"),
        )
    }

    #[cfg(windows)]
    pub fn event_stream(
        &self,
    ) -> impl Stream<Item = std::io::Result<TerminalEvent>> + Unpin + use<> {
        input::Events::new(self.terminal_input.clone().expect("native terminal input"))
    }
}

impl TestApplication {
    pub fn event_stream(
        &self,
    ) -> impl Stream<Item = std::io::Result<TerminalEvent>> + Unpin + use<> {
        futures_util::stream::pending()
    }
}
