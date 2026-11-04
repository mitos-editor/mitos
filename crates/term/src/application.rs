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
    pub interaction: crate::compositor::InteractionState,
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
        self.request_frame();

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
        let mut idle_seen = false;
        loop {
            if self.jobs.wait_futures.is_empty() {
                self.editor.run_after_writes();
            }
            if self.editor.should_close() {
                return false;
            }

            // One input event can enqueue several callbacks. Drain a bounded
            // batch before accepting more input so synchronous event hooks do
            // not spend every keystroke waiting for space in their own queue.
            if self.drain_ready_callbacks() > 0 {
                idle_seen = false;
                if until_idle {
                    self.editor.reset_idle_timer();
                }
            }

            if self.editor.should_close() {
                return false;
            }

            if self.render_state.requested.is_some() {
                self.update_components();
            }
            let mut frame_deadline = self.frame_deadline();
            if frame_deadline.is_some_and(|due| due <= tokio::time::Instant::now()) {
                self.draw_frame();
                frame_deadline = None;
            }

            use futures_util::StreamExt;

            // Idle is reported after its requested frame and accepted work finish.
            if until_idle
                && idle_seen
                && self.render_state.requested.is_none()
                && self.jobs.wait_futures.is_empty()
                && !self.editor.has_pending_writes()
                && !self.editor.has_pending_shell_filters()
                && !self.jobs.command_expansion.is_running()
                && !self.editor.documents().any(|doc| doc.is_syntax_pending())
                && !self.compositor.has_pending_render()
            {
                use futures_util::FutureExt;
                // A cooperative stream may yield with input still queued. This
                // single readiness probe must distinguish that from an empty queue.
                if let Some(Some(event)) =
                    tokio::task::unconstrained(input_stream.next()).now_or_never()
                {
                    self.handle_terminal_events(event).await;
                    idle_seen = false;
                    continue;
                }
                return true;
            }

            // Keep callbacks and saves progressing even when terminal input is
            // continuously ready; input priority can fill the callback queue.
            tokio::select! {
                _ = async {
                    if let Some(due) = frame_deadline { tokio::time::sleep_until(due).await; }
                    else { std::future::pending::<()>().await; }
                } => {},
                Some(signal) = self.signals.next() => {
                    idle_seen = false;
                    if !self.handle_signals(signal).await {
                        return false;
                    };
                }
                Some(request) = self.jobs.interactive_requests.recv() => {
                    idle_seen = false;
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
                    idle_seen = false;
                    self.handle_terminal_events(event).await;
                }
                Some(event) = self.config_updates.1.recv() => {
                    idle_seen = false;
                    self.apply_config_update(event);
                    self.request_frame();
                }
                Some(callback) = self.jobs.callbacks.recv() => {
                    idle_seen = false;
                    self.apply_callback(Ok(Some(callback)));
                }
                Some(callback) = self.jobs.wait_futures.next() => {
                    idle_seen = false;
                    self.apply_callback(callback);
                }
                event = self.editor.wait_event() => {
                    idle_seen = self.handle_editor_event(event).await;
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
        self.editor.cancel_after_writes();
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
                        &mut self.editor,
                        &mut self.interaction,
                        &mut self.compositor,
                        result,
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
