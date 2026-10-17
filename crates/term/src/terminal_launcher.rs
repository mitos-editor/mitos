//! Frontend terminal discovery and the private debuggee launch handoff.
//!
//! A helper runs inside the destination terminal, so a multiplexer server cannot
//! replace the requested working directory or environment with its own defaults.

use crate::config::Config;
use anyhow::{bail, ensure, Context as _};
use arc_swap::ArcSwap;
use dap::requests::{RunInTerminalArguments, RunInTerminalResponse};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

const HELPER_FLAG: &str = "--internal-debug-terminal";
const START_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct TerminalLauncherConfig {
    pub command: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[cfg(windows)]
pub fn default_launcher() -> Option<TerminalLauncherConfig> {
    use stdx::env::{binary_exists, env_var_is_set};

    // zellij has added support for Windows in v0.44.0
    // https://github.com/zellij-org/zellij/releases/tag/v0.44.0
    if env_var_is_set("ZELLIJ") && binary_exists("zellij") {
        return Some(TerminalLauncherConfig {
            command: "zellij".to_string(),
            args: vec!["run".to_string(), "--".to_string()],
        });
    }
    if binary_exists("wt") {
        return Some(TerminalLauncherConfig {
            command: "wt".to_string(),
            args: vec![
                "new-tab".to_string(),
                "--title".to_string(),
                "DEBUG".to_string(),
                "cmd".to_string(),
                "/C".to_string(),
            ],
        });
    }

    Some(TerminalLauncherConfig {
        command: "conhost".to_string(),
        args: vec!["cmd".to_string(), "/C".to_string()],
    })
}

#[cfg(not(any(windows, target_arch = "wasm32")))]
pub fn default_launcher() -> Option<TerminalLauncherConfig> {
    use stdx::env::{binary_exists, env_var_is_set};

    if env_var_is_set("TMUX") && binary_exists("tmux") {
        return Some(TerminalLauncherConfig {
            command: "tmux".to_string(),
            args: vec!["split-window".to_string()],
        });
    }

    if env_var_is_set("ZELLIJ") && binary_exists("zellij") {
        return Some(TerminalLauncherConfig {
            command: "zellij".to_string(),
            args: vec!["run".to_string(), "--".to_string()],
        });
    }

    if env_var_is_set("WEZTERM_UNIX_SOCKET") && binary_exists("wezterm") {
        return Some(TerminalLauncherConfig {
            command: "wezterm".to_string(),
            args: vec!["cli".to_string(), "split-pane".to_string()],
        });
    }

    // Require a socket so remote control does not read responses from the
    // controlling terminal while the editor is using it.
    if let Ok(socket) = std::env::var("KITTY_LISTEN_ON")
        && !socket.is_empty()
        && binary_exists("kitten")
    {
        return Some(TerminalLauncherConfig {
            command: "kitten".to_string(),
            args: vec![
                "@".to_string(),
                "--to".to_string(),
                socket,
                "launch".to_string(),
                "--type=window".to_string(),
                "--".to_string(),
            ],
        });
    }

    if let Ok(socket) = std::env::var("ALACRITTY_SOCKET")
        && !socket.is_empty()
        && binary_exists("alacritty")
    {
        return Some(TerminalLauncherConfig {
            command: "alacritty".to_string(),
            args: vec![
                "msg".to_string(),
                "--socket".to_string(),
                socket,
                "create-window".to_string(),
                "-e".to_string(),
            ],
        });
    }

    None
}

pub(crate) struct ExternalTerminal {
    config: Arc<ArcSwap<Config>>,
    executable: PathBuf,
}

impl ExternalTerminal {
    pub(crate) fn new(config: Arc<ArcSwap<Config>>) -> std::io::Result<Self> {
        Ok(Self {
            config,
            executable: std::env::current_exe()?,
        })
    }
}

impl view::debugger::TerminalLauncher for ExternalTerminal {
    fn launch(
        &self,
        arguments: RunInTerminalArguments,
    ) -> BoxFuture<'static, dap::Result<RunInTerminalResponse>> {
        let launcher = self.config.load().terminal.launcher.clone();
        let executable = self.executable.clone();
        Box::pin(async move {
            launch(launcher, executable, arguments)
                .await
                .map_err(dap::Error::Other)
        })
    }
}

#[derive(Serialize, Deserialize)]
enum Started {
    Process(u32),
    Failed(String),
}

async fn launch(
    launcher: Option<TerminalLauncherConfig>,
    executable: PathBuf,
    arguments: RunInTerminalArguments,
) -> anyhow::Result<RunInTerminalResponse> {
    let launcher = launcher.context("No external terminal configured")?;
    ensure!(!arguments.args.is_empty(), "Debuggee command is empty");
    // Only a private handoff path is passed to the launcher. Environment values
    // and debuggee arguments stay out of the launcher's command line.
    let handoff = tempfile::tempdir()?;
    let request = handoff.path().join("request.json");
    std::fs::write(&request, serde_json::to_vec(&arguments)?)?;
    let mut child = tokio::process::Command::new(&launcher.command)
        .args(&launcher.args)
        .arg(executable)
        .arg(HELPER_FLAG)
        .arg(handoff.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not start external terminal")?;
    let response = handoff.path().join("response.json");
    let started = tokio::time::timeout(START_TIMEOUT, async {
        loop {
            match std::fs::read(&response) {
                Ok(bytes) => {
                    return serde_json::from_slice::<Started>(&bytes).map_err(anyhow::Error::from)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if let Some(status) = child.try_wait()?
                && !status.success()
            {
                bail!("External terminal exited with {status}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("Timed out waiting for the debuggee to start")??;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    match started {
        Started::Process(process_id) => Ok(RunInTerminalResponse {
            process_id: Some(process_id),
            shell_process_id: None,
        }),
        Started::Failed(message) => bail!(message),
    }
}

/// Called before normal CLI initialization; the helper never configures a terminal UI.
pub(crate) fn run_helper() -> Option<anyhow::Result<i32>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HELPER_FLAG)) {
        return None;
    }
    Some((|| {
        let directory = PathBuf::from(args.next().context("Missing debug terminal handoff")?);
        ensure!(
            args.next().is_none(),
            "Unexpected debug terminal helper arguments"
        );
        run_in_terminal(&directory)
    })())
}

fn working_directory(value: &str) -> anyhow::Result<Option<PathBuf>> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.starts_with("file:") {
        return stdx::Url::parse(value)?
            .to_file_path()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("Invalid debuggee working directory URI"));
    }
    Ok(Some(PathBuf::from(value)))
}

fn run_in_terminal(directory: &Path) -> anyhow::Result<i32> {
    let result = (|| {
        let request = directory.join("request.json");
        let arguments: RunInTerminalArguments = serde_json::from_slice(&std::fs::read(&request)?)?;
        std::fs::remove_file(request)?;
        let (program, args) = arguments
            .args
            .split_first()
            .context("Debuggee command is empty")?;
        let mut command = std::process::Command::new(program);
        command.args(args);
        if let Some(cwd) = working_directory(&arguments.cwd)? {
            command.current_dir(cwd);
        }
        for (key, value) in arguments.env.unwrap_or_default() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        command.spawn().context("Could not start debuggee")
    })();
    let response = match &result {
        Ok(child) => Started::Process(child.id()),
        Err(error) => Started::Failed(format!("{error:#}")),
    };
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), &response)?;
    file.persist(directory.join("response.json"))?;
    let status = result?.wait()?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn arguments() -> RunInTerminalArguments {
        RunInTerminalArguments {
            kind: None,
            title: None,
            cwd: String::new(),
            args: vec!["program".into()],
            env: None,
        }
    }

    #[tokio::test]
    async fn missing_launchers_and_empty_commands_fail_without_a_handoff() {
        assert!(launch(None, PathBuf::from("ms"), arguments())
            .await
            .unwrap_err()
            .to_string()
            .contains("No external terminal"));
        let config = TerminalLauncherConfig {
            command: "missing-mitos-terminal-fixture".into(),
            args: vec![],
        };
        let mut empty = arguments();
        empty.args.clear();
        assert!(launch(Some(config.clone()), PathBuf::from("ms"), empty)
            .await
            .unwrap_err()
            .to_string()
            .contains("empty"));
        assert!(launch(Some(config), PathBuf::from("ms"), arguments())
            .await
            .unwrap_err()
            .to_string()
            .contains("Could not start external terminal"));
    }
}
