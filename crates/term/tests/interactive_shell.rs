//! Exercise the real frontend: the integration-feature backend has no terminal.
#![cfg(all(unix, not(feature = "integration")))]

use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

#[path = "support/terminal_screen.rs"]
mod terminal_screen;
use terminal_screen::Screen;

struct Editor {
    child: Child,
    terminal: Option<File>,
    dir: tempfile::TempDir,
    output: String,
    screen: Screen,
}

impl Editor {
    fn new() -> anyhow::Result<Self> {
        Self::with_shell(&["sh", "-c"])
    }

    fn with_shell(shell: &[&str]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let shell = serde_json::to_string(shell)?;
        std::fs::write(
            dir.path().join("config.toml"),
            format!(
                r#"[terminal.keyboard]
kitty-protocol = "enabled"
[editor]
mouse = false
shell = {shell}
"#
            ) + r#"[commands]
":outline" = [":goto %sh_interactive{%arg{0}}", ":echo completed %{cursor_line}"]
":nested" = [":goto %sh_interactive{echo %sh{echo 2}}", ":echo completed %{cursor_line}"]
"#,
        )?;
        std::fs::write(dir.path().join("sample.txt"), "one\ntwo\nthree\nfour\n")?;
        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 640,
            ws_ypixel: 384,
        };
        // SAFETY: openpty initializes both descriptors on success.
        anyhow::ensure!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    // BSD uses a mutable pointer; Linux accepts a const pointer.
                    &raw mut size,
                )
            } == 0,
            "openpty: {}",
            io::Error::last_os_error()
        );
        let terminal = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let fd = slave.as_raw_fd();
        let mut command = Command::new(env!("CARGO_BIN_EXE_ms"));
        command
            .args([
                "--config",
                "config.toml",
                "--log",
                "mitos.log",
                "sample.txt",
            ])
            .current_dir(dir.path())
            .env("TERM", "xterm-256color")
            .env("FZF_DEFAULT_OPTS", "")
            .env_remove("FZF_DEFAULT_OPTS_FILE")
            .env("MITOS_CONFIG", dir.path())
            .env(
                "MITOS_RUNTIME",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtime"),
            )
            .stdin(slave.try_clone()?)
            .stdout(slave.try_clone()?)
            .stderr(slave.try_clone()?);
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 || libc::ioctl(fd, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        drop(slave);
        unsafe { libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK) };
        let mut editor = Self {
            child,
            terminal: Some(terminal),
            dir,
            output: String::new(),
            screen: Screen::default(),
        };
        editor.wait("editor startup", |editor| editor.output.contains("NOR"))?;
        Ok(editor)
    }

    fn send(&mut self, input: &str) -> anyhow::Result<()> {
        self.terminal
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())?;
        Ok(())
    }

    fn drain(&mut self) -> anyhow::Result<()> {
        let mut buffer = [0; 16384];
        loop {
            match self.terminal.as_mut().unwrap().read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let output = String::from_utf8_lossy(&buffer[..count]);
                    // Answer terminal capability probes without requiring a terminal emulator.
                    if output.contains("\x1b[5n") {
                        self.send("\x1b[0n")?;
                    }
                    if output.contains("\x1b[6n") {
                        self.send("\x1b[1;1R")?;
                    }
                    if output.contains("\x1b[16t") {
                        self.send("\x1b[6;16;8t")?;
                    }
                    if output.contains("\x1b[c") {
                        self.send("\x1b[?1;2c")?;
                    }
                    self.output.push_str(&output);
                    self.screen.feed(&output);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn wait(
        &mut self,
        description: &str,
        mut condition: impl FnMut(&Self) -> bool,
    ) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.drain()?;
            if condition(self) {
                return Ok(());
            }
            anyhow::ensure!(
                Instant::now() < deadline && self.child.try_wait()?.is_none(),
                "waiting for {description}; output: {:?}; log: {}",
                self.output
                    .chars()
                    .rev()
                    .take(2000)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>(),
                std::fs::read_to_string(self.dir.path().join("mitos.log")).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn raw(&self) -> bool {
        let mut modes = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.terminal.as_ref().unwrap().as_raw_fd(), &mut modes) },
            0
        );
        modes.c_lflag & libc::ICANON == 0
    }

    fn signal(&self, signal: i32) -> anyhow::Result<()> {
        anyhow::ensure!(
            unsafe { libc::kill(self.child.id() as i32, signal) } == 0,
            "signal {signal}: {}",
            io::Error::last_os_error()
        );
        Ok(())
    }

    fn suspend(&mut self) -> anyhow::Result<()> {
        self.signal(libc::SIGTSTP)?;
        self.wait("editor suspension", |editor| {
            let mut status = 0;
            let pid = unsafe {
                libc::waitpid(
                    editor.child.id() as i32,
                    &mut status,
                    libc::WUNTRACED | libc::WNOHANG,
                )
            };
            pid == editor.child.id() as i32 && libc::WIFSTOPPED(status)
        })?;
        self.drain()?;
        assert!(!self.raw(), "suspension did not restore terminal modes");
        assert!(self.output.contains("\x1b[?1049l"));
        self.output.clear();
        Ok(())
    }

    fn interactive(&mut self, command: &str) -> anyhow::Result<()> {
        self.output.clear();
        self.send(&format!(":outline '{}'\r", command.replace('\'', "''")))
    }

    fn read_line(&mut self) -> anyhow::Result<()> {
        self.read_line_with("")
    }

    fn read_line_with(&mut self, prefix: &str) -> anyhow::Result<()> {
        let _ = std::fs::remove_file(self.dir.path().join("pid"));
        self.interactive(&format!("{prefix}test -t 0 && test -t 2 && ! test -t 1 || exit 9; echo $$ > pid; echo CHILD_READY >&2; read line; echo \"$line\""))?;
        self.wait("interactive input", |editor| {
            editor.dir.path().join("pid").exists()
                && editor.output.contains("CHILD_READY\r\n")
                && !editor.raw()
        })?;
        let group: i32 = std::fs::read_to_string(self.dir.path().join("pid"))?
            .trim()
            .parse()?;
        assert_eq!(
            unsafe { libc::tcgetpgrp(self.terminal.as_ref().unwrap().as_raw_fd()) },
            group
        );
        Ok(())
    }

    fn assert_line(&mut self, line: &str) -> anyhow::Result<()> {
        self.wait("command result", |editor| {
            editor.screen.contains_line(&format!("completed {line}"))
                || editor.screen.contains("Interactive shell command failed")
                || editor.screen.contains("Interactive shell was suspended")
                || editor.screen.contains("Could not start interactive shell")
        })?;
        self.wait("editor terminal reclamation", |editor| editor.raw())?;
        assert_eq!(
            unsafe { libc::tcgetpgrp(self.terminal.as_ref().unwrap().as_raw_fd()) },
            self.child.id() as i32
        );
        // Wait for the editor to execute the continuation, rather than a file
        // written by an expansion whose completion callback may still be queued.
        self.send(":echo cursor-line: %{cursor_line}\r")?;
        self.wait("command continuation", |editor| {
            editor.screen.contains_line(&format!("cursor-line: {line}"))
        })?;
        Ok(())
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let group = unsafe { libc::tcgetpgrp(self.terminal.as_ref().unwrap().as_raw_fd()) };
        if group > 0 && group != self.child.id() as i32 {
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        let _ = self.child.kill();
        // Close the PTY before waiting: terminal teardown can otherwise keep
        // the killed editor from exiting on macOS.
        drop(self.terminal.take());
        let _ = self.child.wait();
    }
}

#[test]
fn suspension_reclaims_terminal_before_processing_queued_events() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    for cycle in 0..16 {
        // Leave input and redraw work ready when the suspend signal wins.
        editor.send(if cycle % 2 == 0 { "j" } else { "k" })?;
        editor.suspend()?;
        let size = libc::winsize {
            ws_row: if cycle % 2 == 0 { 20 } else { 24 },
            ws_col: 80,
            ws_xpixel: 640,
            ws_ypixel: 320,
        };
        assert_eq!(
            unsafe {
                libc::ioctl(
                    editor.terminal.as_ref().unwrap().as_raw_fd(),
                    libc::TIOCSWINSZ as _,
                    &size,
                )
            },
            0
        );
        editor.signal(libc::SIGUSR1)?;
        // Let the idle timer become due while the editor is stopped.
        thread::sleep(Duration::from_millis(300));
        editor.signal(libc::SIGCONT)?;
        editor.wait("resumed editor", |editor| {
            editor.raw() && editor.output.contains("NOR")
        })?;
        assert!(
            editor.output.starts_with("\x1b[?1049h"),
            "cycle {cycle}: editor wrote before reclaiming the alternate screen: {:?}",
            editor.output
        );
        editor.output.clear();
        let status = format!("resumed-{cycle}");
        editor.send(&format!(":echo {status}\r"))?;
        editor.wait("resumed input", |editor| {
            editor.screen.contains_line(&status)
        })?;
    }
    Ok(())
}

#[test]
fn termination_pending_on_resume_exits_the_editor() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.suspend()?;
    editor.signal(libc::SIGTERM)?;
    editor.signal(libc::SIGCONT)?;
    editor.wait("shutdown after suspension", |editor| !editor.raw())?;
    // Waiting for exit, rather than restored modes alone, checks that the
    // exclusive signal wait still honors shutdown.
    let deadline = Instant::now() + Duration::from_secs(10);
    while editor.child.try_wait()?.is_none() {
        anyhow::ensure!(Instant::now() < deadline, "editor did not shut down");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!editor.raw());
    Ok(())
}

#[test]
fn interactive_expansion_owns_input_and_resumes_goto() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.read_line()?;
    assert!(
        editor.output.contains("\x1b[<1u"),
        "keyboard protocol was not released"
    );
    let before = editor.output.clone();
    let resized = libc::winsize {
        ws_row: 20,
        ws_col: 80,
        ws_xpixel: 640,
        ws_ypixel: 320,
    };
    assert_eq!(
        unsafe {
            libc::ioctl(
                editor.terminal.as_ref().unwrap().as_raw_fd(),
                libc::TIOCSWINSZ as _,
                &resized,
            )
        },
        0
    );
    thread::sleep(Duration::from_millis(150));
    editor.drain()?;
    assert_eq!(
        editor.output, before,
        "editor drew over the interactive child"
    );
    editor.send("3\n")?;
    editor.assert_line("3")?;
    assert!(
        editor.output.contains("\x1b[>5u"),
        "keyboard protocol was not reclaimed"
    );

    // Nested shell expansions compose with interactive expansions.
    editor.output.clear();
    editor.send(":nested\r")?;
    editor.assert_line("2")?;
    Ok(())
}

#[test]
fn failures_interrupts_and_stopped_children_restore_the_editor() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.interactive("echo 4; exit 1")?;
    editor.wait("failed child", |editor| {
        editor.screen.contains("Interactive shell command failed")
    })?;
    editor.assert_line("1")?;
    for key in ["\x03", "\x1a"] {
        editor.read_line()?;
        editor.send(key)?;
        editor.assert_line("1")?;
    }
    Ok(())
}

#[test]
fn spawn_failure_restores_terminal_and_editor_input() -> anyhow::Result<()> {
    // Startup configuration is installed before commands run. A queued :set
    // immediately followed by :outline can capture the previous shell instead.
    let mut editor = Editor::with_shell(&["/mitos-missing-shell"])?;
    editor.interactive("echo 4")?;
    editor.wait("spawn failure", |editor| {
        editor.screen.contains("Could not start interactive shell")
    })?;
    editor.assert_line("1")?;
    Ok(())
}

#[test]
fn shutdown_during_interactive_input_restores_terminal_and_kills_the_child() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.read_line_with("sleep 60 >/dev/null 2>&1 & echo $! > grandchild; ")?;
    let grandchild: i32 = std::fs::read_to_string(editor.dir.path().join("grandchild"))?
        .trim()
        .parse()?;
    let group: i32 = std::fs::read_to_string(editor.dir.path().join("pid"))?
        .trim()
        .parse()?;
    unsafe { libc::kill(editor.child.id() as i32, libc::SIGTERM) };
    editor.wait("shutdown", |editor| {
        !editor.raw()
            && unsafe { libc::kill(group, 0) } == -1
            && unsafe { libc::kill(grandchild, 0) } == -1
    })?;
    Ok(())
}

#[test]
#[ignore = "requires fzf in PATH"]
fn fzf_selects_a_line_and_cancels_without_keyboard_protocol_workarounds() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    let command = "printf '1\\n2\\n3\\n' | fzf --height=80% --layout=reverse --no-sort --sync --prompt=Symbol";
    editor.interactive(command)?;
    editor.wait("fzf interface", |editor| editor.screen.contains("3/3"))?;
    editor.send("3")?;
    editor.wait("fzf filtering", |editor| editor.screen.contains("1/3"))?;
    editor.send("\r")?;
    editor.assert_line("3")?;
    editor.interactive(command)?;
    editor.wait("fzf interface", |editor| editor.screen.contains("3/3"))?;
    editor.send("\x1b")?;
    editor.assert_line("3")?;
    Ok(())
}
