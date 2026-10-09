//! Native clipboard detection and command execution.
// Implementation reference: https://github.com/neovim/neovim/blob/f2906a4669a2eef6d7bf86a29648793d63c98949/runtime/autoload/provider/clipboard.vim#L68-L152

use super::config::{Command, CommandProvider};
use super::{ClipboardBackend, ClipboardError, ClipboardProvider, ClipboardType, Result};
use std::borrow::Cow;

mod plugin;

/// Native command and platform clipboard access, without frontend output.
#[derive(Default)]
pub struct NativeClipboard;

impl NativeClipboard {
    /// Bound native frontend IPC without retaining an editor borrow. A blocking
    /// OS operation keeps its permit even if the caller expires.
    pub fn bounded_plugin_ipc<T: Send + 'static>(
        operation: impl FnOnce() -> std::result::Result<T, plugin_api::ServiceError> + Send + 'static,
    ) -> plugin_api::HostFuture<T> {
        plugin::blocking(operation)
    }
}

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

    fn is_exit_success(program: &str, args: &[&str]) -> bool {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .and_then(|out| out.status.success().then_some(()))
            .is_some()
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
impl ClipboardBackend for NativeClipboard {
    fn get_plugin_contents(
        &self,
        provider: ClipboardProvider,
        kind: ClipboardType,
    ) -> plugin_api::HostFuture<String> {
        plugin::read(provider, kind)
    }

    fn set_plugin_contents(
        &self,
        provider: ClipboardProvider,
        content: String,
        kind: ClipboardType,
    ) -> plugin_api::HostFuture<()> {
        plugin::write(provider, content, kind)
    }
    fn name(&self, provider: &ClipboardProvider) -> String {
        fn builtin_name<'a>(
            name: &'static str,
            provider: &'static CommandProvider,
        ) -> Cow<'a, str> {
            if provider.yank.command != provider.paste.command {
                Cow::Owned(format!(
                    "{} ({}+{})",
                    name, provider.yank.command, provider.paste.command
                ))
            } else {
                Cow::Owned(format!("{} ({})", name, provider.yank.command))
            }
        }

        let name: Cow<'_, str> = match provider {
            // These names should match the config option names from Serde
            ClipboardProvider::Pasteboard => builtin_name("pasteboard", &PASTEBOARD),
            ClipboardProvider::Wayland => builtin_name("wayland", &WL_CLIPBOARD),
            ClipboardProvider::XClip => builtin_name("x-clip", &XCLIP),
            ClipboardProvider::XSel => builtin_name("x-sel", &XSEL),
            ClipboardProvider::Win32Yank => builtin_name("win32-yank", &WIN32),
            ClipboardProvider::Tmux => builtin_name("tmux", &TMUX),
            ClipboardProvider::Termux => builtin_name("termux", &TERMUX),
            #[cfg(windows)]
            ClipboardProvider::Windows => "windows".into(),
            ClipboardProvider::Termcode => "termcode".into(),
            ClipboardProvider::Custom(command_provider) => Cow::Owned(format!(
                "custom ({}+{})",
                command_provider.yank.command, command_provider.paste.command
            )),
            ClipboardProvider::None => "none".into(),
        };
        name.into_owned()
    }

    fn get_contents(
        &self,
        provider: &ClipboardProvider,
        clipboard_type: ClipboardType,
    ) -> Result<String> {
        fn yank_from_builtin(
            provider: CommandProvider,
            clipboard_type: ClipboardType,
        ) -> Result<String> {
            match clipboard_type {
                ClipboardType::Clipboard => execute_command(&provider.yank, None, true)?
                    .ok_or(ClipboardError::MissingStdout),
                ClipboardType::Selection => {
                    if let Some(cmd) = provider.yank_primary.as_ref() {
                        return execute_command(cmd, None, true)?
                            .ok_or(ClipboardError::MissingStdout);
                    }

                    Ok(String::new())
                }
            }
        }

        match provider {
            ClipboardProvider::Pasteboard => yank_from_builtin(PASTEBOARD, clipboard_type),
            ClipboardProvider::Wayland => yank_from_builtin(WL_CLIPBOARD, clipboard_type),
            ClipboardProvider::XClip => yank_from_builtin(XCLIP, clipboard_type),
            ClipboardProvider::XSel => yank_from_builtin(XSEL, clipboard_type),
            ClipboardProvider::Win32Yank => yank_from_builtin(WIN32, clipboard_type),
            ClipboardProvider::Tmux => yank_from_builtin(TMUX, clipboard_type),
            ClipboardProvider::Termux => yank_from_builtin(TERMUX, clipboard_type),
            #[cfg(target_os = "windows")]
            ClipboardProvider::Windows => match clipboard_type {
                ClipboardType::Clipboard => {
                    let contents = clipboard_win::get_clipboard(clipboard_win::formats::Unicode)?;
                    Ok(contents)
                }
                ClipboardType::Selection => Ok(String::new()),
            },
            ClipboardProvider::Termcode => Err(ClipboardError::ReadingNotSupported),
            ClipboardProvider::Custom(command_provider) => {
                execute_command(&command_provider.yank, None, true)?
                    .ok_or(ClipboardError::MissingStdout)
            }
            ClipboardProvider::None => Err(ClipboardError::ReadingNotSupported),
        }
    }

    fn set_contents(
        &self,
        provider: &ClipboardProvider,
        content: &str,
        clipboard_type: ClipboardType,
    ) -> Result<()> {
        fn paste_to_builtin(
            provider: CommandProvider,
            content: &str,
            clipboard_type: ClipboardType,
        ) -> Result<()> {
            let cmd = match clipboard_type {
                ClipboardType::Clipboard => &provider.paste,
                ClipboardType::Selection => {
                    if let Some(cmd) = provider.paste_primary.as_ref() {
                        cmd
                    } else {
                        return Ok(());
                    }
                }
            };

            execute_command(cmd, Some(content), false).map(|_| ())
        }

        match provider {
            ClipboardProvider::Pasteboard => paste_to_builtin(PASTEBOARD, content, clipboard_type),
            ClipboardProvider::Wayland => paste_to_builtin(WL_CLIPBOARD, content, clipboard_type),
            ClipboardProvider::XClip => paste_to_builtin(XCLIP, content, clipboard_type),
            ClipboardProvider::XSel => paste_to_builtin(XSEL, content, clipboard_type),
            ClipboardProvider::Win32Yank => paste_to_builtin(WIN32, content, clipboard_type),
            ClipboardProvider::Tmux => paste_to_builtin(TMUX, content, clipboard_type),
            ClipboardProvider::Termux => paste_to_builtin(TERMUX, content, clipboard_type),
            #[cfg(target_os = "windows")]
            ClipboardProvider::Windows => match clipboard_type {
                ClipboardType::Clipboard => {
                    clipboard_win::set_clipboard(clipboard_win::formats::Unicode, content)?;
                    Ok(())
                }
                ClipboardType::Selection => Ok(()),
            },
            ClipboardProvider::Termcode => Err(ClipboardError::Unavailable),
            ClipboardProvider::Custom(command_provider) => match clipboard_type {
                ClipboardType::Clipboard => {
                    execute_command(&command_provider.paste, Some(content), false).map(|_| ())
                }
                ClipboardType::Selection => {
                    if let Some(cmd) = &command_provider.paste_primary {
                        execute_command(cmd, Some(content), false).map(|_| ())
                    } else {
                        Ok(())
                    }
                }
            },
            ClipboardProvider::None => Ok(()),
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

fn execute_command(
    cmd: &Command,
    input: Option<&str>,
    pipe_output: bool,
) -> Result<Option<String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let stdin = input.map(|_| Stdio::piped()).unwrap_or_else(Stdio::null);
    let stdout = pipe_output.then(Stdio::piped).unwrap_or_else(Stdio::null);

    let mut command: Command = Command::new(cmd.command.as_ref());

    #[allow(unused_mut)]
    let mut command_mut: &mut Command = command
        .args(cmd.args.iter().map(AsRef::as_ref))
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::null());

    // Fix for https://github.com/helix-editor/helix/issues/5424
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        unsafe {
            command_mut = command_mut.pre_exec(|| match libc::setsid() {
                -1 => Err(std::io::Error::last_os_error()),
                _ => Ok(()),
            });
        }
    }

    let mut child = command_mut.spawn()?;

    if let Some(input) = input {
        let mut stdin = child.stdin.take().ok_or(ClipboardError::StdinWriteFailed)?;
        stdin
            .write_all(input.as_bytes())
            .map_err(|_| ClipboardError::StdinWriteFailed)?;
    }

    // TODO: add timer?
    let output = child.wait_with_output()?;

    if !output.status.success() {
        log::error!(
            "clipboard provider {} failed with stderr: \"{}\"",
            cmd.command,
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(ClipboardError::CommandFailed);
    }

    if pipe_output {
        Ok(Some(String::from_utf8(output.stdout)?))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
