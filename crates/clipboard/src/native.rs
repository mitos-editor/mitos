//! Native clipboard detection and command execution.
// Implementation reference: https://github.com/neovim/neovim/blob/f2906a4669a2eef6d7bf86a29648793d63c98949/runtime/autoload/provider/clipboard.vim#L68-L152

use super::config::{Command, CommandProvider};
use super::{ClipboardBackend, ClipboardError, ClipboardProvider, ClipboardType, Result};
use std::borrow::Cow;

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
                Some(command) => {
                    execute_command(command, None, true)?.ok_or(ClipboardError::MissingStdout)
                }
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
                Some(command) => execute_command(command, Some(content), false).map(|_| ()),
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
