//! Bounded clipboard IPC. Native editor commands retain their existing behavior.

use super::*;
use plugin_api::{ErrorCode, HostFuture, ServiceError};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
#[cfg(unix)]
use tokio::sync::oneshot;
use tokio::sync::Semaphore;

const LIMIT: usize = 4096;
const DEADLINE: Duration = Duration::from_secs(2);

fn capacity() -> Arc<Semaphore> {
    static CAPACITY: OnceLock<Arc<Semaphore>> = OnceLock::new();
    CAPACITY.get_or_init(|| Arc::new(Semaphore::new(2))).clone()
}

fn error(code: ErrorCode, message: impl Into<String>) -> ServiceError {
    ServiceError::new(code, message)
}

fn command(provider: ClipboardProvider, kind: ClipboardType, write: bool) -> Option<Command> {
    let custom = matches!(provider, ClipboardProvider::Custom(_));
    let commands = match provider {
        ClipboardProvider::Pasteboard => PASTEBOARD,
        ClipboardProvider::Wayland => WL_CLIPBOARD,
        ClipboardProvider::XClip => XCLIP,
        ClipboardProvider::XSel => XSEL,
        ClipboardProvider::Win32Yank => WIN32,
        ClipboardProvider::Tmux => TMUX,
        ClipboardProvider::Termux => TERMUX,
        ClipboardProvider::Custom(commands) => commands,
        _ => return None,
    };
    match (kind, write) {
        (ClipboardType::Clipboard, false) => Some(commands.yank),
        (ClipboardType::Clipboard, true) => Some(commands.paste),
        (ClipboardType::Selection, false) if custom => Some(commands.yank),
        (ClipboardType::Selection, false) => commands.yank_primary,
        (ClipboardType::Selection, true) => commands.paste_primary,
    }
}

pub(super) fn read(provider: ClipboardProvider, kind: ClipboardType) -> HostFuture<String> {
    Box::pin(async move {
        match provider {
            ClipboardProvider::None | ClipboardProvider::Termcode => Err(error(
                ErrorCode::UnsupportedInterface,
                "this clipboard provider cannot read selections",
            )),
            #[cfg(windows)]
            ClipboardProvider::Windows => {
                if kind == ClipboardType::Selection {
                    return Ok(String::new());
                }
                blocking(|| {
                    let _clipboard = clipboard_win::Clipboard::new_attempts(3)
                        .map_err(|e| error(ErrorCode::HostFailure, e.to_string()))?;
                    let size = clipboard_win::raw::size(clipboard_win::formats::CF_UNICODETEXT)
                        .map(usize::from)
                        .unwrap_or(0);
                    if size > (LIMIT + 1) * 2 || size % 2 != 0 {
                        return Err(error(
                            ErrorCode::ResourceExhausted,
                            "clipboard exceeds 4 KiB",
                        ));
                    }
                    let mut bytes = [0; (LIMIT + 1) * 2];
                    let read =
                        clipboard_win::raw::get(clipboard_win::formats::CF_UNICODETEXT, &mut bytes)
                            .map_err(|e| error(ErrorCode::HostFailure, e.to_string()))?;
                    let units: Vec<_> = bytes[..read]
                        .chunks_exact(2)
                        .map(|b| u16::from_le_bytes([b[0], b[1]]))
                        .take_while(|u| *u != 0)
                        .collect();
                    let text = String::from_utf16(&units).map_err(|_| {
                        error(ErrorCode::HostFailure, "clipboard is not valid Unicode")
                    })?;
                    if text.len() > LIMIT {
                        return Err(error(
                            ErrorCode::ResourceExhausted,
                            "clipboard exceeds 4 KiB",
                        ));
                    }
                    Ok(text)
                })
                .await
            }
            provider => match command(provider, kind, false) {
                Some(command) => run(command, None).await,
                None => Ok(String::new()),
            },
        }
    })
}

pub(super) fn write(
    provider: ClipboardProvider,
    content: String,
    kind: ClipboardType,
) -> HostFuture<()> {
    Box::pin(async move {
        if content.len() > LIMIT {
            return Err(error(
                ErrorCode::ResourceExhausted,
                "clipboard exceeds 4 KiB",
            ));
        }
        match provider {
            ClipboardProvider::Termcode => Err(error(
                ErrorCode::UnsupportedInterface,
                "terminal clipboard needs frontend output",
            )),
            ClipboardProvider::None => Ok(()),
            #[cfg(windows)]
            ClipboardProvider::Windows => {
                if kind == ClipboardType::Selection {
                    return Ok(());
                }
                blocking(move || {
                    let _clipboard = clipboard_win::Clipboard::new_attempts(3)
                        .map_err(|e| error(ErrorCode::HostFailure, e.to_string()))?;
                    clipboard_win::raw::set_string(&content)
                        .map_err(|e| error(ErrorCode::HostFailure, e.to_string()))
                })
                .await
            }
            provider => match command(provider, kind, true) {
                Some(command) => run(command, Some(content)).await.map(|_| ()),
                None => Ok(()),
            },
        }
    })
}

/// Keep the permit inside blocking IPC even when its caller expires. Native OS
/// clipboard/stdout calls do not offer a hard interrupt; at most two may remain.
pub(crate) fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> std::result::Result<T, ServiceError> + Send + 'static,
) -> HostFuture<T> {
    Box::pin(async move {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        let permit = tokio::time::timeout_at(deadline, capacity().acquire_owned())
            .await
            .map_err(|_| error(ErrorCode::DeadlineExceeded, "clipboard admission expired"))?
            .map_err(|_| error(ErrorCode::Cancelled, "clipboard service stopped"))?;
        tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                operation()
            }),
        )
        .await
        .map_err(|_| error(ErrorCode::DeadlineExceeded, "clipboard IPC expired"))?
        .map_err(|_| error(ErrorCode::HostFailure, "clipboard IPC failed"))?
    })
}

#[cfg(unix)]
struct CancelOnDrop(Option<oneshot::Sender<()>>);
#[cfg(unix)]
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            let _ = cancel.send(());
        }
    }
}

#[cfg(unix)]
async fn run(command: Command, input: Option<String>) -> std::result::Result<String, ServiceError> {
    use std::process::Stdio;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let permit = tokio::time::timeout_at(deadline, capacity().acquire_owned())
        .await
        .map_err(|_| error(ErrorCode::DeadlineExceeded, "clipboard admission expired"))?
        .map_err(|_| error(ErrorCode::Cancelled, "clipboard service stopped"))?;
    let (cancel, mut cancelled) = oneshot::channel();
    let _cancel = CancelOnDrop(Some(cancel));
    let (result, completed) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let work = async {
            let mut provider = tokio::process::Command::new(command.command.as_ref());
            provider.args(command.args.iter().map(AsRef::as_ref))
                .env_clear().process_group(0).kill_on_drop(true)
                .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
                .stdout(if input.is_none() { Stdio::piped() } else { Stdio::null() })
                .stderr(Stdio::null());
            for key in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "TMUX"] {
                if let Some(value) = std::env::var_os(key) { provider.env(key, value); }
            }
            let mut child = provider.spawn()
                .map_err(|e| error(ErrorCode::HostFailure, format!("clipboard provider: {e}")))?;
            let pid = child.id();
            let stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let mut reaped = false;
            let io = async {
                let write = async {
                    if let (Some(mut stdin), Some(input)) = (stdin, input) {
                        stdin.write_all(input.as_bytes()).await?;
                        stdin.shutdown().await?;
                    }
                    Ok::<_, std::io::Error>(())
                };
                let read = async {
                    let mut bytes = Vec::with_capacity(LIMIT + 1);
                    if let Some(stdout) = stdout { stdout.take((LIMIT + 1) as u64).read_to_end(&mut bytes).await?; }
                    Ok::<_, std::io::Error>(bytes)
                };
                let (_, bytes) = tokio::try_join!(write, read)
                    .map_err(|e| error(ErrorCode::HostFailure, format!("clipboard IPC: {e}")))?;
                if bytes.len() > LIMIT { return Err(error(ErrorCode::ResourceExhausted, "clipboard exceeds 4 KiB")); }
                let text = String::from_utf8(bytes)
                    .map_err(|_| error(ErrorCode::HostFailure, "clipboard is not valid UTF-8"))?;
                // Keep the leader unreaped while inherited pipes remain open.
                let status = child.wait().await.map_err(|e| error(ErrorCode::HostFailure, e.to_string()))?;
                reaped = true;
                if !status.success() {
                    return Err(error(ErrorCode::HostFailure, "clipboard provider failed"));
                }
                Ok(text)
            };
            let outcome = tokio::select! {
                result = io => result,
                _ = &mut cancelled => Err(error(ErrorCode::Cancelled, "clipboard request cancelled")),
                _ = tokio::time::sleep_until(deadline) => Err(error(ErrorCode::DeadlineExceeded, "clipboard IPC expired")),
            };
            if outcome.is_err() && !reaped {
                if let Some(pid) = pid {
                    // The unreaped leader pins the group ID through cancellation.
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL); }
                }
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            outcome
        }.await;
        let _ = result.send(work);
    });
    completed
        .await
        .map_err(|_| error(ErrorCode::HostFailure, "clipboard worker stopped"))?
}

#[cfg(not(unix))]
async fn run(_: Command, _: Option<String>) -> std::result::Result<String, ServiceError> {
    Err(error(
        ErrorCode::UnsupportedInterface,
        "clipboard command cancellation is unavailable on this platform",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        Command {
            command: Cow::Borrowed("/bin/sh"),
            args: Cow::Owned(vec![Cow::Borrowed("-c"), Cow::Owned(script.to_owned())]),
        }
    }

    #[tokio::test]
    async fn clipboard_commands_bound_output_and_report_failure() {
        assert_eq!(run(shell("printf 'α\\n'"), None).await.unwrap(), "α\n");
        assert_eq!(
            run(shell("exit 1"), None).await.unwrap_err().code,
            ErrorCode::HostFailure
        );
        assert_eq!(
            run(shell("while :; do printf 0123456789; done"), None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceExhausted
        );
        assert_eq!(
            write(
                ClipboardProvider::None,
                "x".repeat(LIMIT + 1),
                ClipboardType::Clipboard
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::ResourceExhausted
        );
    }

    #[tokio::test]
    async fn cancellation_reaps_provider_and_releases_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!(
            "printf '%s' $$ > '{}'; exec /bin/sleep 30",
            pid_file.display()
        );
        let task = tokio::spawn(run(shell(&script), None));
        let pid = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(value) = tokio::fs::read_to_string(&pid_file).await
                    && let Ok(pid) = value.parse::<i32>()
                {
                    break pid;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(run(shell("printf done"), None).await.unwrap(), "done");
    }
}
