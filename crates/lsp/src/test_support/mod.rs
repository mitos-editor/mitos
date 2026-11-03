//! Explicit process fixtures for protocol integration tests. Excluded from default builds.
mod server;
use anyhow::Context as _;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn run() -> anyhow::Result<()> {
    server::run()
}

/// A file gate keeps server startup under the test's control.
pub struct Gate(PathBuf);

impl Gate {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn close(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(&self.0) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn release(&self) -> anyhow::Result<()> {
        std::fs::write(&self.0, "ready")?;
        Ok(())
    }
}

pub fn log_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}.jsonl"))
}

/// Read complete JSON records while the fixture may still be appending to its log.
/// An unfinished trailing record can also end in the middle of a UTF-8 character.
pub fn read_log(path: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    std::fs::read(path)?
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|record| record.ends_with(b"\n"))
        .map(serde_json::from_slice)
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

#[test]
fn log_snapshots_wait_for_complete_records_and_report_invalid_json() -> anyhow::Result<()> {
    use std::io::Write;
    let mut log = tempfile::NamedTempFile::new()?;
    log.write_all(b"{\"phase\":\"ready\"}\n{\"phase\":\"\xe7")?;
    assert_eq!(
        read_log(log.path())?,
        vec![serde_json::json!({"phase": "ready"})]
    );
    log.write_all(b"\x95\x8c\"}\n")?;
    assert_eq!(
        read_log(log.path())?,
        vec![
            serde_json::json!({"phase": "ready"}),
            serde_json::json!({"phase": "界"})
        ]
    );
    log.write_all(b"invalid\n")?;
    assert!(read_log(log.path()).is_err());
    Ok(())
}

/// Configure the existing stdio test server. Feature modes require the mode,
/// server name, and log path in that order; additional arguments stay explicit.
pub struct ServerConfig<'a> {
    name: &'a str,
    args: Vec<String>,
}

impl<'a> ServerConfig<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            args: Vec::new(),
        }
    }

    pub fn feature(name: &'a str, mode: &str, directory: &Path) -> Self {
        Self::new(name)
            .arg(mode)
            .arg(name)
            .arg(log_path(directory, name))
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(
            arg.as_ref()
                .to_str()
                .expect("UTF-8 test server argument")
                .into(),
        );
        self
    }

    pub fn initialize_gate(self, gate: &Gate) -> Self {
        self.arg("--initialize-gate").arg(gate.path())
    }

    pub fn toml(self, executable: &str) -> String {
        let command = toml::Value::String(executable.into());
        let args = string_array(&self.args);
        let name = toml::Value::String(self.name.into());
        format!("[language-server.{name}]\ncommand = {command}\nargs = {args}\n")
    }
}

fn string_array(values: &[impl AsRef<str>]) -> toml::Value {
    toml::Value::Array(
        values
            .iter()
            .map(|value| toml::Value::String(value.as_ref().into()))
            .collect(),
    )
}

pub async fn wait_message(path: &Path, method: &str) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records = match read_log(path) {
                Ok(records) => records,
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    Vec::new()
                }
                Err(error) => return Err(error),
            };
            if let Some(message) = records
                .into_iter()
                .find(|message| message["method"] == method)
            {
                return Ok(message);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .with_context(|| format!("server did not receive {method}"))?
}

pub fn language_config(language: &str, file_type: &str, names: &[&str], servers: &str) -> String {
    let name = toml::Value::String(language.into());
    let scope = toml::Value::String(format!("source.{language}"));
    let file_types = string_array(&[file_type]);
    let names = string_array(names);
    format!(
        r#"
        {servers}
        [[language]]
        name = {name}
        scope = {scope}
        file-types = {file_types}
        roots = []
        language-servers = {names}
        "#
    )
}
