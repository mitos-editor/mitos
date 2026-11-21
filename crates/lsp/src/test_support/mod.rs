//! Optional real-process fixtures shared by editor and frontend integration tests.
mod server;
pub use server::run;

use std::path::{Path, PathBuf};

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
