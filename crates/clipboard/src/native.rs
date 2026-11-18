//! Native clipboard detection and command execution.
// Implementation reference: https://github.com/neovim/neovim/blob/f2906a4669a2eef6d7bf86a29648793d63c98949/runtime/autoload/provider/clipboard.vim#L68-L152

use super::config::{Command, CommandProvider};
use super::{ClipboardBackend, ClipboardError, ClipboardProvider, ClipboardType, Result};
use std::borrow::Cow;

#[cfg(windows)]
mod windows;

/// Native command and platform clipboard access, without frontend output.
#[derive(Default)]
pub struct NativeClipboard;

#[cfg(windows)]
pub(super) fn default_provider() -> ClipboardProvider {
    use stdx::env::binary_exists;

    if binary_exists("win32yank.exe") {
        ClipboardProvider::Win32Yank
    } else {
        ClipboardProvider::Windows
    }
}

#[cfg(target_os = "macos")]
pub(super) fn default_provider() -> ClipboardProvider {
    use stdx::env::{binary_exists, env_var_is_set};

    if env_var_is_set("TMUX") && binary_exists("tmux") {
        ClipboardProvider::Tmux
    } else if binary_exists("pbcopy") && binary_exists("pbpaste") {
        ClipboardProvider::Pasteboard
    } else {
        ClipboardProvider::None
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn default_provider() -> ClipboardProvider {
    use stdx::env::{binary_exists, env_var_is_set};

    fn is_exit_success(program: &'static str, args: &[&'static str]) -> bool {
        let command = Command {
            command: Cow::Borrowed(program),
            args: Cow::Owned(args.iter().map(|arg| Cow::Borrowed(*arg)).collect()),
        };
        execute_command(&command, None).is_ok()
    }

    if binary_exists("termux-clipboard-set") && binary_exists("termux-clipboard-get") {
        ClipboardProvider::Termux
    } else if env_var_is_set("TMUX") && binary_exists("tmux") {
        ClipboardProvider::Tmux
    } else if env_var_is_set("WAYLAND_DISPLAY")
        && binary_exists("wl-copy")
        && binary_exists("wl-paste")
    {
        ClipboardProvider::Wayland
    } else if env_var_is_set("DISPLAY") && binary_exists("xclip") {
        ClipboardProvider::XClip
    } else if env_var_is_set("DISPLAY")
        && binary_exists("xsel")
        // FIXME: check performance of is_exit_success
        && is_exit_success("xsel", &["-o", "-b"])
    {
        ClipboardProvider::XSel
    } else if binary_exists("win32yank.exe") {
        ClipboardProvider::Win32Yank
    } else {
        ClipboardProvider::None
    }
}
fn commands(provider: &ClipboardProvider) -> Option<(&str, &CommandProvider)> {
    Some(match provider {
        ClipboardProvider::Pasteboard => ("pasteboard", &PASTEBOARD),
        ClipboardProvider::Wayland => ("wayland", &WL_CLIPBOARD),
        ClipboardProvider::XClip => ("x-clip", &XCLIP),
        ClipboardProvider::XSel => ("x-sel", &XSEL),
        ClipboardProvider::Win32Yank => ("win32-yank", &WIN32),
        ClipboardProvider::Tmux => ("tmux", &TMUX),
        ClipboardProvider::Termux => ("termux", &TERMUX),
        ClipboardProvider::Custom(commands) => ("custom", commands),
        _ => return None,
    })
}

impl ClipboardBackend for NativeClipboard {
    fn name(&self, provider: &ClipboardProvider) -> String {
        if let Some((name, commands)) = commands(provider) {
            return if name == "custom" || commands.yank.command != commands.paste.command {
                format!(
                    "{name} ({}+{})",
                    commands.yank.command, commands.paste.command
                )
            } else {
                format!("{name} ({})", commands.yank.command)
            };
        }
        match provider {
            #[cfg(windows)]
            ClipboardProvider::Windows => "windows",
            ClipboardProvider::Termcode => "termcode",
            ClipboardProvider::None => "none",
            _ => unreachable!("command providers are handled above"),
        }
        .into()
    }

    fn get_contents(&self, provider: &ClipboardProvider, kind: ClipboardType) -> Result<String> {
        if let Some((_, commands)) = commands(provider) {
            let command = match (kind, provider) {
                (ClipboardType::Clipboard, _) | (_, ClipboardProvider::Custom(_)) => {
                    Some(&commands.yank)
                }
                (ClipboardType::Selection, _) => commands.yank_primary.as_ref(),
            };
            return match command {
                Some(command) => read_command(command, None),
                None => Ok(String::new()),
            };
        }
        match provider {
            #[cfg(windows)]
            ClipboardProvider::Windows => match kind {
                ClipboardType::Clipboard => Ok(clipboard_win::get_clipboard(
                    clipboard_win::formats::Unicode,
                )?),
                ClipboardType::Selection => Ok(String::new()),
            },
            ClipboardProvider::Termcode | ClipboardProvider::None => {
                Err(ClipboardError::ReadingNotSupported)
            }
            _ => unreachable!("command providers are handled above"),
        }
    }

    fn set_contents(
        &self,
        provider: &ClipboardProvider,
        content: &str,
        kind: ClipboardType,
    ) -> Result<()> {
        if let Some((_, commands)) = commands(provider) {
            let command = match kind {
                ClipboardType::Clipboard => Some(&commands.paste),
                ClipboardType::Selection => commands.paste_primary.as_ref(),
            };
            return match command {
                Some(command) => execute_command(command, Some(content)),
                None => Ok(()),
            };
        }
        match provider {
            #[cfg(windows)]
            ClipboardProvider::Windows => match kind {
                ClipboardType::Clipboard => Ok(clipboard_win::set_clipboard(
                    clipboard_win::formats::Unicode,
                    content,
                )?),
                ClipboardType::Selection => Ok(()),
            },
            ClipboardProvider::Termcode => Err(ClipboardError::Unavailable),
            ClipboardProvider::None => Ok(()),
            _ => unreachable!("command providers are handled above"),
        }
    }
}

macro_rules! command_provider {
    ($name:ident,
     yank => $yank_cmd:literal $( , $yank_arg:literal )* ;
     paste => $paste_cmd:literal $( , $paste_arg:literal )* ; ) => {
        const $name: CommandProvider = CommandProvider {
            yank: Command {
                command: Cow::Borrowed($yank_cmd),
                args: Cow::Borrowed(&[ $( Cow::Borrowed($yank_arg) ),* ])
            },
            paste: Command {
                command: Cow::Borrowed($paste_cmd),
                args: Cow::Borrowed(&[ $( Cow::Borrowed($paste_arg) ),* ])
            },
            yank_primary: None,
            paste_primary: None,
        };
    };
    ($name:ident,
     yank => $yank_cmd:literal $( , $yank_arg:literal )* ;
     paste => $paste_cmd:literal $( , $paste_arg:literal )* ;
     yank_primary => $yank_primary_cmd:literal $( , $yank_primary_arg:literal )* ;
     paste_primary => $paste_primary_cmd:literal $( , $paste_primary_arg:literal )* ; ) => {
        const $name: CommandProvider = CommandProvider {
            yank: Command {
                command: Cow::Borrowed($yank_cmd),
                args: Cow::Borrowed(&[ $( Cow::Borrowed($yank_arg) ),* ])
            },
            paste: Command {
                command: Cow::Borrowed($paste_cmd),
                args: Cow::Borrowed(&[ $( Cow::Borrowed($paste_arg) ),* ])
            },
            yank_primary: Some(Command {
                command: Cow::Borrowed($yank_primary_cmd),
                args: Cow::Borrowed(&[ $( Cow::Borrowed($yank_primary_arg) ),* ])
            }),
            paste_primary: Some(Command {
                command: Cow::Borrowed($paste_primary_cmd),
                args: Cow::Borrowed(&[ $( Cow::Borrowed($paste_primary_arg) ),* ])
            }),
        };
    };
}

command_provider! {
    TMUX,
    yank => "tmux", "save-buffer", "-";
    paste => "tmux", "load-buffer", "-w", "-";
}
command_provider! {
    PASTEBOARD,
    yank => "pbpaste";
    paste => "pbcopy";
}
command_provider! {
    WL_CLIPBOARD,
    yank => "wl-paste", "--no-newline";
    paste => "wl-copy", "--type", "text/plain";
    yank_primary => "wl-paste", "-p", "--no-newline";
    paste_primary => "wl-copy", "-p", "--type", "text/plain";
}
command_provider! {
    XCLIP,
    yank => "xclip", "-o", "-selection", "clipboard";
    paste => "xclip", "-i", "-selection", "clipboard";
    yank_primary => "xclip", "-o";
    paste_primary => "xclip", "-i";
}
command_provider! {
    XSEL,
    yank => "xsel", "-o", "-b";
    paste => "xsel", "-i", "-b";
    yank_primary => "xsel", "-o";
    paste_primary => "xsel", "-i";
}
command_provider! {
    WIN32,
    yank => "win32yank.exe", "-o", "--lf";
    paste => "win32yank.exe", "-i", "--crlf";
}
command_provider! {
    TERMUX,
    yank => "termux-clipboard-get";
    paste => "termux-clipboard-set";
}

#[derive(Clone, Copy)]
enum CommandOutput {
    Capture,
    Discard,
}

fn execute_command(cmd: &Command, input: Option<&str>) -> Result<()> {
    execute_command_with_timeout(
        cmd,
        input,
        CommandOutput::Discard,
        std::time::Duration::from_secs(5),
    )
    .map(|_| ())
}

fn read_command(cmd: &Command, input: Option<&str>) -> Result<String> {
    execute_command_with_timeout(
        cmd,
        input,
        CommandOutput::Capture,
        std::time::Duration::from_secs(5),
    )
}

fn execute_command_with_timeout(
    cmd: &Command,
    input: Option<&str>,
    output: CommandOutput,
    timeout: std::time::Duration,
) -> Result<String> {
    // This synchronous API is also called inside the editor's Tokio runtime.
    // A separate thread lets cancellable pipe I/O use its own runtime without
    // nesting block_on or leaving blocked pipe workers behind on timeout.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn_scoped(scope, || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?
                    .block_on(execute_command_async(cmd, input, output, timeout))
            })?
            .join()
            .map_err(|_| ClipboardError::CommandFailed)?
    })
}

async fn execute_command_async(
    cmd: &Command,
    input: Option<&str>,
    output: CommandOutput,
    timeout: std::time::Duration,
) -> Result<String> {
    use std::process::{Command, Stdio};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let pipe_output = matches!(output, CommandOutput::Capture);
    let mut command: Command = Command::new(cmd.command.as_ref());

    command
        .args(cmd.args.iter().map(AsRef::as_ref))
        .stderr(Stdio::null());

    #[cfg(not(windows))]
    command
        .stdin(input.map(|_| Stdio::piped()).unwrap_or_else(Stdio::null))
        .stdout(pipe_output.then(Stdio::piped).unwrap_or_else(Stdio::null));

    // Tokio's anonymous Windows pipes use blocking I/O workers. Overlapped
    // named pipes allow cancellation even when another process inherits an end.
    #[cfg(windows)]
    let (mut stdin, mut stdout) = {
        let stdin = if input.is_some() {
            let (pipe, child_end) = windows::pipe(true).await?;
            command.stdin(child_end);
            Some(pipe)
        } else {
            command.stdin(Stdio::null());
            None
        };
        let stdout = if pipe_output {
            let (pipe, child_end) = windows::pipe(false).await?;
            command.stdout(child_end);
            Some(pipe)
        } else {
            command.stdout(Stdio::null());
            None
        };
        (stdin, stdout)
    };

    // Fix for https://github.com/helix-editor/helix/issues/5424
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        unsafe {
            command.pre_exec(|| match libc::setsid() {
                -1 => Err(std::io::Error::last_os_error()),
                _ => Ok(()),
            });
        }
    }

    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()?;
    #[cfg(not(windows))]
    let mut stdin = child.stdin.take();
    #[cfg(not(windows))]
    let mut stdout = child.stdout.take();
    let result = tokio::time::timeout(timeout, async {
        let write = async {
            if let Some(input) = input {
                let stdin = stdin.as_mut().ok_or(ClipboardError::StdinWriteFailed)?;
                stdin
                    .write_all(input.as_bytes())
                    .await
                    .map_err(|_| ClipboardError::StdinWriteFailed)?;
                #[cfg(windows)]
                windows::finish_write(stdin)
                    .await
                    .map_err(|_| ClipboardError::StdinWriteFailed)?;
            }
            drop(stdin.take());
            Ok::<_, ClipboardError>(())
        };
        let read = async {
            let mut output = Vec::new();
            if let Some(stdout) = stdout.as_mut() {
                stdout.read_to_end(&mut output).await?;
            }
            Ok::<_, ClipboardError>(output)
        };
        let wait = async { child.wait().await.map_err(ClipboardError::from) };
        let ((), output, status) = tokio::try_join!(write, read, wait)?;
        if !status.success() {
            log::error!("clipboard provider {} failed: {status}", cmd.command);
            return Err(ClipboardError::CommandFailed);
        }
        if pipe_output {
            Ok(String::from_utf8(output)?)
        } else {
            Ok(String::new())
        }
    })
    .await
    .unwrap_or(Err(ClipboardError::CommandTimedOut));

    if result.is_err() {
        // Cancel Windows writes explicitly: Mio otherwise flushes them on drop.
        #[cfg(windows)]
        for pipe in stdin.iter().chain(stdout.iter()) {
            windows::cancel(pipe);
        }
        drop((stdin, stdout));
        // kill also waits, so the helper is reaped.
        child.kill().await?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn shell_command(script: &str, path: &std::path::Path) -> Command {
        serde_json::from_value(serde_json::json!({
            "command": "/bin/sh", "args": ["-c", script, "clipboard-test", path]
        }))
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn helper_timeout_terminates_and_reaps_process() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("pid");
        let command = shell_command("printf '%s' $$ > \"$1\"; exec sleep 5", &pid_path);
        let started = std::time::Instant::now();
        let result = execute_command_with_timeout(
            &command,
            None,
            CommandOutput::Capture,
            std::time::Duration::from_millis(500),
        );
        assert!(
            matches!(result, Err(ClipboardError::CommandTimedOut)),
            "{result:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let pid: libc::pid_t = std::fs::read_to_string(pid_path).unwrap().parse().unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "helper is still alive");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_timeout_covers_blocked_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell_command("exec sleep 5", dir.path());
        let input = "x".repeat(8 * 1024 * 1024);
        let started = std::time::Instant::now();
        let result = execute_command_with_timeout(
            &command,
            Some(&input),
            CommandOutput::Discard,
            std::time::Duration::from_millis(500),
        );
        assert!(
            matches!(result, Err(ClipboardError::CommandTimedOut)),
            "{result:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn helper_timeout_covers_inherited_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell_command("sleep 5 & printf ready", dir.path());
        let started = std::time::Instant::now();
        let result = execute_command_with_timeout(
            &command,
            None,
            CommandOutput::Capture,
            std::time::Duration::from_millis(500),
        );
        assert!(
            matches!(result, Err(ClipboardError::CommandTimedOut)),
            "{result:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn large_duplex_io_works_inside_a_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell_command("exec cat", dir.path());
        let input = "α\n".repeat(512 * 1024);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert_eq!(read_command(&command, Some(&input)).unwrap(), input);
        });
    }

    #[cfg(windows)]
    fn self_command(helper: &str) -> Command {
        serde_json::from_value(serde_json::json!({
            "command": std::env::current_exe().unwrap(),
            "args": ["--exact", helper, "--ignored", "--nocapture"]
        }))
        .unwrap()
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture"]
    fn clipboard_helper_wait() {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture"]
    #[allow(clippy::zombie_processes)] // The fixture must exit before its child closes stdout.
    fn clipboard_helper_inherit_stdout() {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native::tests::clipboard_helper_wait",
                "--ignored",
                "--nocapture",
            ])
            .spawn()
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture"]
    fn clipboard_helper_echo() {
        std::io::copy(&mut std::io::stdin().lock(), &mut std::io::stdout().lock()).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_deadline_covers_blocked_input_output_and_exit() {
        let input = "x".repeat(8 * 1024 * 1024);
        for (helper, input, output) in [
            (
                "native::tests::clipboard_helper_wait",
                None,
                CommandOutput::Capture,
            ),
            (
                "native::tests::clipboard_helper_wait",
                Some(input.as_str()),
                CommandOutput::Discard,
            ),
            (
                "native::tests::clipboard_helper_inherit_stdout",
                None,
                CommandOutput::Capture,
            ),
        ] {
            let started = std::time::Instant::now();
            let result = execute_command_with_timeout(
                &self_command(helper),
                input,
                output,
                std::time::Duration::from_millis(500),
            );
            assert!(
                matches!(result, Err(ClipboardError::CommandTimedOut)),
                "{result:?}"
            );
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipes_preserve_large_unicode_input_and_output() {
        let input = "α\n".repeat(512 * 1024);
        let output = read_command(
            &self_command("native::tests::clipboard_helper_echo"),
            Some(&input),
        )
        .unwrap();
        // The Rust test harness writes its own framing around the helper output.
        assert!(output.contains(&input));
    }

    #[test]
    fn terminal_output_requires_a_frontend_backend() {
        assert!(matches!(
            NativeClipboard.set_contents(
                &ClipboardProvider::Termcode,
                "text",
                ClipboardType::Clipboard
            ),
            Err(ClipboardError::Unavailable)
        ));
        assert!(matches!(
            NativeClipboard.get_contents(&ClipboardProvider::Termcode, ClipboardType::Clipboard),
            Err(ClipboardError::ReadingNotSupported)
        ));
        assert!(NativeClipboard
            .set_contents(&ClipboardProvider::None, "text", ClipboardType::Selection)
            .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn custom_commands_keep_selection_behavior_and_report_failures() {
        let dir = tempfile::tempdir().unwrap();
        let clipboard = dir.path().join("clipboard");
        let primary = dir.path().join("primary");
        let command = |script: &str, path: &std::path::Path| {
            serde_json::json!({
                "command": "/bin/sh", "args": ["-c", script, "clipboard-test", path]
            })
        };
        let provider: ClipboardProvider = serde_json::from_value(serde_json::json!({"custom": {
            "yank": command("cat \"$1\"", &clipboard),
            "paste": command("cat > \"$1\"", &clipboard),
            "yank-primary": command("cat \"$1\"", &primary),
            "paste-primary": command("cat > \"$1\"", &primary)
        }}))
        .unwrap();
        let text = "α\nsecond line\n";
        NativeClipboard
            .set_contents(&provider, text, ClipboardType::Clipboard)
            .unwrap();
        NativeClipboard
            .set_contents(&provider, "primary", ClipboardType::Selection)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&clipboard).unwrap(), text);
        assert_eq!(std::fs::read_to_string(&primary).unwrap(), "primary");
        // Custom providers historically read through `yank` for both selections.
        for kind in [ClipboardType::Clipboard, ClipboardType::Selection] {
            assert_eq!(NativeClipboard.get_contents(&provider, kind).unwrap(), text);
        }
        let provider: ClipboardProvider = serde_json::from_value(serde_json::json!({"custom": {
            "yank": { "command": "/bin/sh", "args": ["-c", "exit 7"] },
            "paste": command("cat > \"$1\"", &clipboard)
        }}))
        .unwrap();
        assert!(matches!(
            NativeClipboard.get_contents(&provider, ClipboardType::Clipboard),
            Err(ClipboardError::CommandFailed)
        ));
        NativeClipboard
            .set_contents(&provider, "ignored", ClipboardType::Selection)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&clipboard).unwrap(), text);
    }
}
