//! Unix signals and exclusive terminal handoff while the editor is suspended.

use futures_util::{Stream, StreamExt};
use signal_hook::consts::signal;
use tui::backend::BackendExt;
use view::graphics::Rect;

use super::Application;

impl Application {
    pub async fn handle_signals(&mut self, signal: i32) -> bool {
        match signal {
            signal::SIGTSTP => return self.suspend().await,
            signal::SIGCONT => {
                self.reclaim_term();
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

    async fn suspend(&mut self) -> bool {
        #[cfg(not(feature = "integration"))]
        let paused = self.terminal_input.pause();
        self.restore_term().unwrap();

        // SAFETY: The editor can signal its own process group. Stop the whole
        // group so a parent waiting for Mitos (for example, git commit) also
        // yields the terminal. SIGSTOP avoids the signal-hook/Tokio issue at
        // https://github.com/vorner/signal-hook/issues/132.
        let res = unsafe { libc::kill(0, signal::SIGSTOP) };
        if res != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!("{}", err);
            std::process::exit(err.raw_os_error().unwrap_or(1));
        }

        // Continue owning event dispatch until the terminal is reclaimed.
        // Returning to the main loop here would let ready callbacks, input,
        // resize events or overdue timers write to the shell's screen before
        // that loop selects SIGCONT.
        let Some(resume) = wait_for_resume(&mut self.signals).await else {
            return false;
        };
        self.reclaim_term();
        if resume.refresh_config {
            self.refresh_config();
        }
        #[cfg(not(feature = "integration"))]
        drop(paused);
        self.render().await;
        true
    }

    fn reclaim_term(&mut self) {
        // Claiming can transiently fail immediately after continuation.
        // https://github.com/neovim/neovim/pull/13084
        for retries in 1..=10 {
            match self.terminal.backend_mut().claim() {
                Ok(()) => break,
                Err(err) if retries == 10 => panic!("Failed to claim terminal: {}", err),
                Err(_) => continue,
            }
        }

        let area = Rect::from(self.terminal.size().expect("failed to read terminal size"));
        self.compositor.resize(area);
        self.compositor.need_full_redraw();
    }
}

#[derive(Default)]
struct Resume {
    refresh_config: bool,
}

async fn wait_for_resume(signals: &mut (impl Stream<Item = i32> + Unpin)) -> Option<Resume> {
    let mut resume = Resume::default();
    while let Some(signal) = signals.next().await {
        match signal {
            signal::SIGCONT => return Some(resume),
            signal::SIGTERM | signal::SIGINT => return None,
            // Reloading can change terminal modes and colors, so defer it
            // until the terminal belongs to the editor again.
            signal::SIGUSR1 => resume.refresh_config = true,
            signal::SIGTSTP => {}
            _ => unreachable!(),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    #[tokio::test]
    async fn suspension_waits_for_continue_and_defers_config_refresh() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut signals = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        tx.send(signal::SIGTSTP).unwrap();
        tx.send(signal::SIGUSR1).unwrap();
        let mut waiting = std::pin::pin!(wait_for_resume(&mut signals));
        assert!(waiting.as_mut().now_or_never().is_none());

        tx.send(signal::SIGCONT).unwrap();
        assert!(waiting.await.unwrap().refresh_config);
    }

    #[tokio::test]
    async fn shutdown_does_not_require_terminal_reclamation() {
        for signal in [signal::SIGINT, signal::SIGTERM] {
            let mut signals = futures_util::stream::iter([signal::SIGUSR1, signal]);
            assert!(wait_for_resume(&mut signals).await.is_none());
        }
        assert!(wait_for_resume(&mut futures_util::stream::empty())
            .await
            .is_none());
    }
}
