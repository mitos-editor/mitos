//! Terminal clipboard selection and OSC 52 output.
use std::io::Write;
use view::clipboard::{
    ClipboardBackend, ClipboardProvider, ClipboardType, NativeClipboard, Result,
};

pub(crate) struct TerminalClipboard;

impl ClipboardBackend for TerminalClipboard {
    fn get_plugin_contents(
        &self,
        provider: ClipboardProvider,
        kind: ClipboardType,
    ) -> plugin_api::HostFuture<String> {
        NativeClipboard.get_plugin_contents(provider, kind)
    }

    fn set_plugin_contents(
        &self,
        provider: ClipboardProvider,
        content: String,
        kind: ClipboardType,
    ) -> plugin_api::HostFuture<()> {
        if matches!(provider, ClipboardProvider::Termcode) {
            NativeClipboard::bounded_plugin_ipc(move || {
                if content.len() > 4096 {
                    return Err(plugin_api::ServiceError::new(
                        plugin_api::ErrorCode::ResourceExhausted,
                        "clipboard exceeds 4 KiB",
                    ));
                }
                write_selection(&mut std::io::stdout().lock(), &content, kind).map_err(|error| {
                    plugin_api::ServiceError::new(
                        plugin_api::ErrorCode::HostFailure,
                        error.to_string(),
                    )
                })
            })
        } else {
            NativeClipboard.set_plugin_contents(provider, content, kind)
        }
    }
    fn name(&self, provider: &ClipboardProvider) -> String {
        NativeClipboard.name(provider)
    }
    fn get_contents(&self, provider: &ClipboardProvider, kind: ClipboardType) -> Result<String> {
        NativeClipboard.get_contents(provider, kind)
    }
    fn set_contents(
        &self,
        provider: &ClipboardProvider,
        content: &str,
        kind: ClipboardType,
    ) -> Result<()> {
        if matches!(provider, ClipboardProvider::Termcode) {
            write_selection(&mut std::io::stdout().lock(), content, kind)?;
            Ok(())
        } else {
            NativeClipboard.set_contents(provider, content, kind)
        }
    }
}

fn write_selection(
    output: &mut impl Write,
    content: &str,
    kind: ClipboardType,
) -> std::io::Result<()> {
    use termina::escape::osc::{self, Osc};
    let selection = match kind {
        ClipboardType::Clipboard => osc::Selection::CLIPBOARD,
        ClipboardType::Selection => osc::Selection::PRIMARY,
    };
    write!(output, "{}", Osc::SetSelection(selection, content))?;
    output.flush()
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn default_provider() -> ClipboardProvider {
    match ClipboardProvider::default() {
        ClipboardProvider::None => ClipboardProvider::Termcode,
        provider => provider,
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn default_provider() -> ClipboardProvider {
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
    } else if env_var_is_set("WEZTERM_UNIX_SOCKET") && binary_exists("wezterm") {
        ClipboardProvider::Termcode
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
        ClipboardProvider::Termcode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use view::clipboard::ClipboardError;

    #[derive(Default)]
    struct Output {
        bytes: Vec<u8>,
        flushes: usize,
        fail_flush: bool,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            if self.fail_flush {
                Err(std::io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn osc52_selects_clipboard_or_primary_and_flushes() {
        let mut output = Output::default();
        write_selection(&mut output, "hi", ClipboardType::Clipboard).unwrap();
        write_selection(&mut output, "hi", ClipboardType::Selection).unwrap();
        write_selection(&mut output, "", ClipboardType::Clipboard).unwrap();
        assert_eq!(
            output.bytes,
            b"\x1b]52;c;aGk=\x1b\\\x1b]52;p;aGk=\x1b\\\x1b]52;c;\x1b\\"
        );
        assert_eq!(output.flushes, 3);
        output.fail_flush = true;
        assert_eq!(
            write_selection(&mut output, "hi", ClipboardType::Clipboard)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn terminal_reads_keep_register_fallback_and_none_does_not_output() {
        assert!(matches!(
            TerminalClipboard.get_contents(&ClipboardProvider::Termcode, ClipboardType::Selection),
            Err(ClipboardError::ReadingNotSupported)
        ));
        TerminalClipboard
            .set_contents(
                &ClipboardProvider::None,
                "ignored",
                ClipboardType::Clipboard,
            )
            .unwrap();
        assert_eq!(
            TerminalClipboard.name(&ClipboardProvider::Termcode),
            "termcode"
        );
    }

    // Run detection in a child so PATH/session variables cannot affect parallel tests.
    #[cfg(unix)]
    #[test]
    fn detection_and_config_defaults_preserve_provider_precedence() {
        use crate::config::{Config, ConfigLoadError, EditorSettings};
        const MARKER: &str = "MITOS_TEST_CLIPBOARD_EXPECTED";
        if let Ok(expected) = std::env::var(MARKER) {
            let provider: ClipboardProvider =
                serde_json::from_str(&format!("\"{expected}\"")).unwrap();
            assert_eq!(default_provider(), provider);
            assert_ne!(ClipboardProvider::default(), ClipboardProvider::Termcode);
            assert_eq!(Config::default().editor.clipboard_provider, provider);
            assert_eq!(
                EditorSettings::default().editor.clipboard_provider,
                provider
            );
            for source in ["", "[editor]", "[editor]\nscrolloff = 7"] {
                let config =
                    Config::load(Ok(&source.to_owned()), Err(ConfigLoadError::default())).unwrap();
                assert_eq!(config.editor.clipboard_provider, provider);
            }
            for explicit in ["none", "termcode"] {
                let source = format!("[editor]\nclipboard-provider = '{explicit}'");
                let config = Config::load(Ok(&source), Err(ConfigLoadError::default())).unwrap();
                assert_eq!(
                    serde_json::to_value(config.editor.clipboard_provider).unwrap(),
                    explicit
                );
            }
            return;
        }
        fn check(programs: &[&str], env: &[&str], expected: &str) {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            for program in programs {
                let path = dir.path().join(program);
                std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "clipboard::tests::detection_and_config_defaults_preserve_provider_precedence",
                    "--nocapture",
                ])
                .env("PATH", dir.path())
                .env(MARKER, expected);
            for key in ["TMUX", "WEZTERM_UNIX_SOCKET", "WAYLAND_DISPLAY", "DISPLAY"] {
                command.env_remove(key);
            }
            for key in env {
                command.env(key, "test");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{expected}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        check(&[], &[], "termcode");
        check(&["tmux"], &["TMUX"], "tmux");
        #[cfg(target_os = "macos")]
        {
            check(&["pbcopy", "pbpaste"], &[], "pasteboard");
            check(&["tmux", "pbcopy", "pbpaste"], &["TMUX"], "tmux");
        }
        #[cfg(not(target_os = "macos"))]
        {
            let programs = [
                "termux-clipboard-set",
                "termux-clipboard-get",
                "tmux",
                "wezterm",
                "wl-copy",
                "wl-paste",
                "xclip",
                "xsel",
                "win32yank.exe",
            ];
            let env = ["TMUX", "WEZTERM_UNIX_SOCKET", "WAYLAND_DISPLAY", "DISPLAY"];
            check(&programs, &env, "termux");
            check(&programs[2..], &env, "tmux");
            check(&programs[3..], &env, "termcode");
            check(&programs[4..], &env, "wayland");
            check(&programs[6..], &env, "x-clip");
            check(&programs[7..], &env, "x-sel");
            check(&programs[8..], &env, "win32-yank");
        }
    }
}
