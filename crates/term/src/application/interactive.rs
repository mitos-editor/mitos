//! Exclusive terminal handoff for command argument expansion.

use super::Application;
use crate::job::InteractiveShellRequest;
#[cfg(not(feature = "integration"))]
use futures_util::StreamExt;
#[cfg(not(feature = "integration"))]
use tui::backend::BackendExt;

impl Application {
    pub(super) async fn interactive_shell(
        &mut self,
        request: InteractiveShellRequest,
    ) -> anyhow::Result<bool> {
        if request.result.is_closed() {
            return Ok(true);
        }
        #[cfg(feature = "integration")]
        {
            drop(request.command);
            let _ = request.result.send(Err(anyhow::anyhow!(
                "interactive shell expansion requires a real terminal"
            )));
            Ok(true)
        }
        #[cfg(not(feature = "integration"))]
        {
            let mut request = request;
            let paused = self.terminal_input.pause();
            // Install before releasing raw mode and retain through reclamation:
            // console Ctrl-c is also delivered to Mitos while the child runs.
            #[cfg(windows)]
            let _control = match process::ControlHandler::install() {
                Ok(control) => control,
                Err(error) => {
                    let _ = request.result.send(Err(error.into()));
                    return Ok(true);
                }
            };
            let mut signal = None;
            let output = match self.restore_term() {
                Ok(()) => {
                    // Do not process editor input, callbacks or renders until the
                    // child has returned terminal ownership. Dropping execution
                    // cleans up its processes and restores console ownership.
                    tokio::select! {
                        result = process::run(request.command) => result,
                        _ = request.result.closed() => Err(anyhow::anyhow!("Command cancelled")),
                        received = self.signals.next() => {
                            signal = received;
                            Err(anyhow::anyhow!("Interactive shell interrupted"))
                        }
                    }
                }
                Err(error) => Err(error.into()),
            };
            // Always reclaim modes and input, including spawn failure and cancellation.
            let claimed = self.terminal.backend_mut().claim();
            drop(paused);
            claimed?;
            let area = view::graphics::Rect::from(self.terminal.size()?);
            self.compositor.resize(area);
            self.compositor.need_full_redraw();
            let _ = request.result.send(output);
            if let Some(signal) = signal {
                return Ok(self.handle_signals(signal).await);
            }
            self.render().await;
            Ok(true)
        }
    }
}

#[cfg(all(windows, not(feature = "integration")))]
#[path = "interactive/windows.rs"]
mod process;

#[cfg(all(not(windows), not(feature = "integration")))]
mod process {
    use anyhow::{ensure, Context};
    use futures_util::StreamExt;
    use std::{fs::File, io, os::fd::AsRawFd, process::Stdio};

    /// Changing the foreground group from the background would otherwise stop
    /// the editor with SIGTTOU. Block it only on this thread, for this syscall.
    fn foreground(fd: libc::c_int, group: libc::pid_t) -> io::Result<()> {
        unsafe {
            let mut mask = std::mem::zeroed();
            let mut previous = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGTTOU);
            let error = libc::pthread_sigmask(libc::SIG_BLOCK, &mask, &mut previous);
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error));
            }
            let result = libc::tcsetpgrp(fd, group);
            let error = io::Error::last_os_error();
            libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
            if result == -1 {
                return Err(error);
            }
        }
        Ok(())
    }

    struct Terminal {
        tty: File,
        group: libc::pid_t,
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            if let Err(error) = foreground(self.tty.as_raw_fd(), self.group) {
                log::error!("Could not restore terminal foreground process group: {error}");
            }
        }
    }

    struct ProcessGroup(libc::pid_t);

    impl Drop for ProcessGroup {
        fn drop(&mut self) {
            // A shell may have spawned pipelines or a TUI grandchild. Killing
            // just the shell would leave those children attached to our terminal.
            unsafe { libc::kill(-self.0, libc::SIGKILL) };
        }
    }

    pub(super) async fn run(
        mut command: tokio::process::Command,
    ) -> anyhow::Result<std::process::Output> {
        use std::os::unix::process::CommandExt;

        let tty = File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("Could not open the controlling terminal")?;
        let fd = tty.as_raw_fd();
        let group = unsafe { libc::tcgetpgrp(fd) };
        ensure!(
            group >= 0,
            "Could not read terminal foreground process group: {}",
            io::Error::last_os_error()
        );
        let terminal = Terminal { tty, group };
        command
            .stdin(terminal.tty.try_clone()?)
            .stderr(terminal.tty.try_clone()?)
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        // The child claims the foreground before exec, avoiding a race in which
        // its first terminal read receives SIGTTIN. This closure uses only
        // async-signal-safe syscalls, with no allocation or locks.
        unsafe {
            command.pre_exec(move || foreground(fd, libc::getpid()));
        }
        let mut child_signals = signal_hook_tokio::Signals::new([libc::SIGCHLD])?;
        let child = command
            .spawn()
            .context("Could not start interactive shell")?;
        let group = ProcessGroup(child.id().expect("spawned child has an id") as libc::pid_t);
        let output = child.wait_with_output();
        tokio::pin!(output);
        let result = loop {
            tokio::select! {
                output = &mut output => break output,
                _ = child_signals.next() => {
                    // We have no suspended-job UI. Recover the terminal instead
                    // of waiting forever if the child is stopped (e.g. Ctrl-z).
                    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
                    let status = unsafe {
                        libc::waitid(libc::P_PID, group.0 as _, &mut info,
                            libc::WSTOPPED | libc::WNOHANG | libc::WNOWAIT)
                    };
                    if status == 0 && unsafe { info.si_pid() } != 0 {
                        anyhow::bail!("Interactive shell was suspended");
                    }
                }
            }
        };
        drop(group);
        drop(terminal);
        Ok(result?)
    }
}
