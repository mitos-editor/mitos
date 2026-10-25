//! Frontend state, event loop, signals, and shutdown.
//! Private adapters own startup, configuration, protocol dispatch, and rendering.

use arc_swap::ArcSwap;
use futures_util::Stream;
use tui::backend::BackendExt;
#[cfg(not(windows))]
use view::graphics::Rect;
use view::{editor::ConfigEvent, Editor};

mod configuration;
mod events;
#[cfg(all(not(windows), not(feature = "integration")))]
mod input;
mod interactive;
mod lsp;
mod render;
mod startup;

use crate::{compositor::Compositor, config::Config, job::Jobs};

use std::sync::Arc;

use anyhow::Error;

#[cfg(not(windows))]
use {signal_hook::consts::signal, signal_hook_tokio::Signals};
#[cfg(windows)]
type Signals = futures_util::stream::Empty<()>;

#[cfg(all(not(windows), not(feature = "integration")))]
use tui::backend::TerminaBackend;

#[cfg(all(windows, not(feature = "integration")))]
use tui::backend::CrosstermBackend;

#[cfg(feature = "integration")]
use tui::backend::TestBackend;

#[cfg(all(not(windows), not(feature = "integration")))]
type TerminalBackend = TerminaBackend;
#[cfg(all(windows, not(feature = "integration")))]
type TerminalBackend = CrosstermBackend<std::io::Stdout>;
#[cfg(feature = "integration")]
type TerminalBackend = TestBackend;

#[cfg(not(windows))]
type TerminalEvent = termina::Event;
#[cfg(windows)]
type TerminalEvent = crossterm::event::Event;

type Terminal = tui::Terminal<TerminalBackend>;

pub struct Application {
    compositor: Compositor,
    terminal: Terminal,
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
    #[cfg(all(not(windows), not(feature = "integration")))]
    terminal_input: input::Control,
    _presentation_hooks: event::HookRegistration,

    theme_mode: Option<theme::Mode>,
}

impl Application {
    /// Screen and cursor state exposed to the integration test harness.
    #[cfg(feature = "integration")]
    pub fn terminal_backend(&self) -> &TestBackend {
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
            if !self.event_loop_until_idle(input_stream).await {
                break;
            }
        }
    }

    pub async fn event_loop_until_idle<S>(&mut self, input_stream: &mut S) -> bool
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
            if self.drain_ready_callbacks() {
                self.render().await;
                #[cfg(feature = "integration")]
                self.editor.reset_idle_timer();
                // Callbacks can close the editor as well as change its state.
                if self.editor.should_close() {
                    return false;
                }
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
                    match event {
                        crate::config::ConfigEvent::Update(settings) => {
                            let old_editor_config = self.editor.config();
                            self.apply_editor_settings(*settings);
                            self.editor.refresh_config(&old_editor_config);
                        }
                        crate::config::ConfigEvent::Refresh => {
                            self.handle_config_events(ConfigEvent::Refresh);
                        }
                    }
                    self.render().await;
                }
                Some(callback) = self.jobs.callbacks.recv() => {
                    if let Some(job) = self.jobs.handle_callback(crate::config::Context { current: &self.config.load(), updates: &self.config_updates.0 }, &mut self.editor, &mut self.compositor, Ok(Some(callback))) {
                        self.jobs.add(job);
                    }
                    self.render().await;
                }
                Some(callback) = self.jobs.wait_futures.next() => {
                    if let Some(job) = self.jobs.handle_callback(crate::config::Context { current: &self.config.load(), updates: &self.config_updates.0 }, &mut self.editor, &mut self.compositor, callback) {
                        self.jobs.add(job);
                    }
                    self.render().await;
                }
                event = self.editor.wait_event() => {
                    let _idle_handled = self.handle_editor_event(event).await;

                    #[cfg(feature = "integration")]
                    {
                        // Don't report idle while a save is still in flight, or an assertion on the post-save document state (e.g. its path,
                        // set in `handle_document_write`) can run before the `DocumentSavedEvent` is processed. Slow file I/O on Windows
                        // (atomic_save's rename/fsync dance over the still-open temp file) makes this race observable.
                        // Initial syntax must also settle before assertions; its completion can
                        // schedule spelling checks. Selection shell results must also settle before
                        // command assertions inspect their edits. Completions arrive via jobs.
                        if _idle_handled
                            && self.jobs.wait_futures.is_empty()
                            && !self.editor.has_pending_writes()
                            && !self.editor.has_pending_shell_filters()
                            && !self.jobs.command_expansion.is_running()
                            && !self.editor.documents().any(|doc| doc.is_syntax_pending())
                            && !self.compositor.has_pending_render()
                        {
                            return true;
                        }
                    }
                }
            }

            // for integration tests only, reset the idle timer after every
            // event to signal when test events are done processing
            #[cfg(feature = "integration")]
            {
                self.editor.reset_idle_timer();
            }
        }
    }

    #[cfg(windows)]
    // no signal handling available on windows
    pub async fn handle_signals(&mut self, _signal: ()) -> bool {
        true
    }

    #[cfg(not(windows))]
    pub async fn handle_signals(&mut self, signal: i32) -> bool {
        match signal {
            signal::SIGTSTP => {
                self.restore_term().unwrap();

                // SAFETY:
                //
                // - mitos must have permissions to send signals to all processes in its signal
                //   group, either by already having the requisite permission, or by having the
                //   user's UID / EUID / SUID match that of the receiving process(es).
                let res = unsafe {
                    // A pid of 0 sends the signal to the entire process group, allowing the user to
                    // regain control of their terminal if the editor was spawned under another process
                    // (e.g. when running `git commit`).
                    //
                    // We have to send SIGSTOP (not SIGTSTP) to the entire process group, because,
                    // as mentioned above, the terminal will get stuck if `mitos` was spawned from
                    // an external process and that process waits for `mitos` to complete. This may
                    // be an issue with signal-hook-tokio, but the author of signal-hook believes it
                    // could be a tokio issue instead:
                    // https://github.com/vorner/signal-hook/issues/132
                    libc::kill(0, signal::SIGSTOP)
                };

                if res != 0 {
                    let err = std::io::Error::last_os_error();
                    eprintln!("{}", err);
                    let res = err.raw_os_error().unwrap_or(1);
                    std::process::exit(res);
                }
            }
            signal::SIGCONT => {
                // Copy/Paste from same issue from neovim:
                // https://github.com/neovim/neovim/issues/12322
                // https://github.com/neovim/neovim/pull/13084
                for retries in 1..=10 {
                    match self.terminal.backend_mut().claim() {
                        Ok(()) => break,
                        Err(err) if retries == 10 => panic!("Failed to claim terminal: {}", err),
                        Err(_) => continue,
                    }
                }

                // redraw the terminal
                let area = Rect::from(self.terminal.size().expect("failed to read terminal size"));
                self.compositor.resize(area);
                self.compositor.need_full_redraw();

                self.render().await;
            }
            signal::SIGUSR1 => {
                self.refresh_config();
                self.render().await;
            }
            signal::SIGTERM | signal::SIGINT => {
                self.restore_term().unwrap();
                return false;
            }
            _ => unreachable!(),
        }

        true
    }

    fn restore_term(&mut self) -> std::io::Result<()> {
        self.terminal.show_cursor().ok();
        self.terminal.backend_mut().restore()
    }

    #[cfg(all(not(feature = "integration"), not(windows)))]
    pub fn event_stream(
        &self,
    ) -> impl Stream<Item = std::io::Result<TerminalEvent>> + Unpin + use<> {
        use termina::Terminal as _;
        input::Events::new(
            self.terminal.backend().terminal().event_reader(),
            self.terminal_input.clone(),
        )
    }

    #[cfg(all(not(feature = "integration"), windows))]
    pub fn event_stream(
        &self,
    ) -> impl Stream<Item = std::io::Result<TerminalEvent>> + Unpin + use<> {
        crossterm::event::EventStream::new()
    }

    #[cfg(feature = "integration")]
    pub fn event_stream(
        &self,
    ) -> impl Stream<Item = std::io::Result<TerminalEvent>> + Unpin + use<> {
        use std::{
            pin::Pin,
            task::{Context, Poll},
        };

        /// A dummy stream that never polls as ready.
        pub struct DummyEventStream;

        impl Stream for DummyEventStream {
            type Item = std::io::Result<TerminalEvent>;

            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                Poll::Pending
            }
        }

        DummyEventStream
    }

    pub async fn run<S>(&mut self, input_stream: &mut S) -> Result<i32, Error>
    where
        S: Stream<Item = std::io::Result<TerminalEvent>> + Unpin,
    {
        self.terminal.backend_mut().claim()?;

        #[cfg(not(feature = "integration"))]
        {
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

        #[cfg(feature = "integration")]
        {
            self.image_picker = None;
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
                        Ok(Some(job)) => self.jobs.add(job),
                        Ok(None) => (),
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

#[cfg(all(test, feature = "integration"))]
mod save_tests;
