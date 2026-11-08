//! Git fallback boundary: pin repositories, filter inherited overrides, and bound output.
use std::{
    io::Read,
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

use anyhow::{bail, ensure, Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt};

const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Access {
    ReadOnly,
    Mutation,
}

#[derive(Clone, Copy)]
pub(crate) enum Output {
    Display,
    Private(&'static str),
}

pub(crate) fn system(access: Access) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "log.showSignature=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // git rev-parse --local-env-vars: never inherit another process's repository,
    // temporary index/object store, or command-line configuration overrides.
    for name in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
        "GIT_GLOB_PATHSPECS",
        "GIT_NOGLOB_PATHSPECS",
        "GIT_ICASE_PATHSPECS",
        "GIT_LITERAL_PATHSPECS",
    ] {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_CONFIG_KEY_")
            || name.to_string_lossy().starts_with("GIT_CONFIG_VALUE_")
        {
            command.env_remove(name);
        }
    }
    if matches!(access, Access::ReadOnly) {
        command.env("GIT_NO_LAZY_FETCH", "1");
    }
    command
}

pub(crate) fn repository(root: &Path, git_dir: &Path, access: Access) -> Command {
    let mut command = system(access);
    // Git for Windows rejects Rust's verbatim paths. Keep native canonical paths
    // for identity checks and simplify them only at the subprocess boundary.
    command
        .current_dir(dunce::simplified(root))
        .arg("--git-dir")
        .arg(dunce::simplified(git_dir))
        .arg("--work-tree")
        .arg(dunce::simplified(root));
    command
}

/// Retain a bounded prefix while draining both pipes to avoid blocking the child.
async fn drain_async(mut reader: impl AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let retained = read.min(OUTPUT_LIMIT - bytes.len());
        bytes.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((bytes, truncated))
}

fn drain_sync(mut reader: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > OUTPUT_LIMIT;
    bytes.truncate(OUTPUT_LIMIT);
    std::io::copy(&mut reader, &mut std::io::sink())?;
    Ok((bytes, truncated))
}

fn finish(
    status: ExitStatus,
    stdout: (Vec<u8>, bool),
    stderr: (Vec<u8>, bool),
    policy: Output,
) -> Result<Vec<u8>> {
    let (stdout, out_truncated) = stdout;
    let (stderr, err_truncated) = stderr;
    if !status.success() {
        if let Output::Private(label) = policy {
            bail!("{label} failed ({status}). Check repository state and Git authentication in your terminal; command output is hidden because it can contain credentials");
        }
        let message = if stderr.is_empty() { &stdout } else { &stderr };
        let message: String = String::from_utf8_lossy(message)
            .chars()
            .take(2000)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        bail!(
            "Git failed ({status}): {}{}",
            message.trim(),
            if out_truncated || err_truncated {
                " (output truncated)"
            } else {
                ""
            }
        );
    }
    ensure!(!out_truncated, "Git output exceeds the 1 MiB display limit");
    Ok(stdout)
}

async fn execute(mut command: tokio::process::Command, policy: Output) -> Result<Vec<u8>> {
    let mut child = command
        .kill_on_drop(true)
        .spawn()
        .context("Could not start Git; install git and ensure it is on PATH")?;
    let stdout = child.stdout.take().context("Git stdout was not piped")?;
    let stderr = child.stderr.take().context("Git stderr was not piped")?;
    let (status, stdout, stderr) =
        tokio::join!(child.wait(), drain_async(stdout), drain_async(stderr));
    finish(status?, stdout?, stderr?, policy)
}

pub(crate) async fn run(command: tokio::process::Command) -> Result<Vec<u8>> {
    execute(command, Output::Display).await
}

pub(crate) async fn run_private(
    command: tokio::process::Command,
    label: &'static str,
) -> Result<Vec<u8>> {
    execute(command, Output::Private(label)).await
}

pub(crate) fn run_sync(command: Command, policy: Output) -> Result<Vec<u8>> {
    let (status, stdout, stderr) = synchronous(command, |stdout| Ok(drain_sync(stdout)?))?;
    finish(status, stdout?, stderr, policy)
}

/// Parse structured stdout as it arrives instead of applying the display-output cap.
/// The consumer must bound its own working buffer. Stderr remains capped and concurrent,
/// and unread stdout is drained even when parsing fails so the child can always finish.
pub(crate) fn read_sync<T>(
    command: Command,
    consume: impl FnOnce(&mut dyn Read) -> Result<T>,
) -> Result<T> {
    let (status, result, stderr) = synchronous(command, consume)?;
    finish(status, (Vec::new(), false), stderr, Output::Display)?;
    result
}

type Captured = (Vec<u8>, bool);

fn synchronous<T>(
    mut command: Command,
    consume: impl FnOnce(&mut dyn Read) -> Result<T>,
) -> Result<(ExitStatus, Result<T>, Captured)> {
    let mut child = command
        .spawn()
        .context("Could not start Git; install git and ensure it is on PATH")?;
    let mut stdout = child.stdout.take().context("Git stdout was not piped")?;
    let stderr = child.stderr.take().context("Git stderr was not piped")?;
    std::thread::scope(|scope| {
        let stderr = scope.spawn(move || drain_sync(stderr));
        let result = consume(&mut stdout);
        let drained = std::io::copy(&mut stdout, &mut std::io::sink());
        let status = child.wait();
        let stderr = stderr
            .join()
            .map_err(|_| anyhow::anyhow!("Git stderr reader panicked"))?;
        drained?;
        Ok((status?, result, stderr?))
    })
}

#[cfg(test)]
mod tests;
