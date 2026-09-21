//! Terminal application and interactive UI for Mitos.
//!
//! This is the composition layer of the editor. [`application`] owns startup
//! and the main event loop, [`commands`] mutates the backend-independent
//! `view::Editor`, and [`ui`] renders that state through the compositor. LSP,
//! DAP, jobs, and typed event hooks feed results back into the same event loop;
//! they must not mutate terminal UI state directly from background tasks.
//!
//! The crate also contains the `mitos` executable, but keeps most behavior in
//! this library so integration tests can drive commands and editor state
//! without owning a real terminal.

#[macro_use]
extern crate view;

pub mod application;
pub mod args;
pub mod commands;
pub mod compositor;
pub mod config;
pub mod events;
pub mod health;
pub mod job;
pub mod keymap;
pub mod logging;
pub mod ui;

#[cfg(not(windows))]
use std::env::var_os;

use std::path::Path;
use std::process::Stdio;

use futures_util::Future;
mod handlers;

use ignore::DirEntry;
use stdx::Url;

#[cfg(windows)]
fn true_color() -> bool {
    true
}

#[cfg(not(windows))]
fn true_color() -> bool {
    if var_os("COLORTERM").is_some_and(|v| v == "truecolor" || v == "24bit")
        || var_os("WSL_DISTRO_NAME").is_some()
    {
        return true;
    }

    match termini::TermInfo::from_env() {
        Ok(t) => {
            t.extended_cap("RGB").is_some()
                || t.extended_cap("Tc").is_some()
                || (t.extended_cap("setrgbf").is_some() && t.extended_cap("setrgbb").is_some())
        }
        Err(_) => false,
    }
}

pub(crate) use stdx::file::is_binary;

/// Function used for filtering dir entries in the various file pickers.
fn filter_picker_entry(entry: &DirEntry, root: &Path, dedup_symlinks: bool) -> bool {
    // We always want to ignore popular VCS directories, otherwise if
    // `ignore` is turned off, we end up with a lot of noise
    // in our picker.
    if matches!(
        entry.file_name().to_str(),
        Some(".git" | ".pijul" | ".jj" | ".hg" | ".svn")
    ) {
        return false;
    }

    // We also ignore symlinks that point inside the current directory
    // if `dedup_links` is enabled.
    if dedup_symlinks && entry.path_is_symlink() {
        return entry
            .path()
            .canonicalize()
            .ok()
            .is_some_and(|path| !path.starts_with(root));
    }

    true
}

/// Opens URL in external program.
fn open_external_url_callback(
    url: Url,
) -> impl Future<Output = Result<job::Callback, anyhow::Error>> + Send + 'static {
    let commands = open::commands(url.as_str());
    async {
        for cmd in commands {
            let mut command: tokio::process::Command = cmd.into();
            command.stdin(Stdio::null());
            let output = match command.output().await {
                Ok(output) => output,
                Err(err) => {
                    log::debug!("Failed to launch external URL opener: {err}");
                    continue;
                }
            };
            if output.status.success() {
                return Ok(job::Callback::Editor(Box::new(|_| {})));
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            log::warn!(
                "External URL opener exited with status {}. stdout: {:?}, stderr: {:?}",
                output.status,
                stdout.trim(),
                stderr.trim()
            );
        }
        Ok(job::Callback::Editor(Box::new(move |editor| {
            editor.set_error(|| "Opening URL in external program failed")
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::is_binary;

    #[test]
    fn binary_detection() {
        assert!(!is_binary(b""));
        assert!(!is_binary(b"plain text\nsecond line"));
        // a NUL byte in the scanned range -> binary
        assert!(is_binary(b"text\0with nul"));
        // binary magic numbers with no NUL prefix
        assert!(is_binary(b"%PDF-1.7 ..."));
        assert!(is_binary(b"\x89PNG\r\n"));
        // a BOM marks the content as text even though it carries NUL bytes
        assert!(!is_binary(b"\xFF\xFEt\0e\0x\0t\0")); // UTF-16LE
        assert!(!is_binary(b"\x00\x00\xFE\xFFtext")); // UTF-32BE
        assert!(!is_binary(b"\xEF\xBB\xBFtext")); // UTF-8 BOM
    }
}
