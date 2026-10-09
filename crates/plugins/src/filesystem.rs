//! File access relative to an already granted directory handle.

use std::{
    io::Read,
    path::{Component, Path},
};

use anyhow::{ensure, Context};
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};

pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

/// A root selected by the host, never by a guest-supplied absolute path.
pub struct ScopedDirectory {
    dir: Dir,
}

impl ScopedDirectory {
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            dir: Dir::open_ambient_dir(root, cap_std::ambient_authority())
                .with_context(|| format!("opening granted directory {}", root.display()))?,
        })
    }

    pub fn read(&self, path: &Path) -> anyhow::Result<Vec<u8>> {
        self.read_bounded(path, MAX_FILE_BYTES)
    }

    pub(crate) fn read_bounded(&self, path: &Path, limit: usize) -> anyhow::Result<Vec<u8>> {
        relative_path(path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK);
        let file = self.dir.open_with(path, &options)?;
        ensure!(
            file.metadata()?.is_file(),
            "plugin path is not a regular file"
        );
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= limit, "plugin file exceeds the read limit");
        Ok(bytes)
    }

    pub fn write(&self, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        relative_path(path)?;
        ensure!(
            bytes.len() <= MAX_FILE_BYTES,
            "plugin file exceeds the write limit"
        );
        let mut options = OpenOptions::new();
        options.write(true).create(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK);
        let mut file = self.dir.open_with(path, &options)?;
        ensure!(
            file.metadata()?.is_file(),
            "plugin path is not a regular file"
        );
        file.set_len(0)?;
        std::io::Write::write_all(&mut file, bytes)?;
        Ok(())
    }

    pub fn directory(&self) -> &Dir {
        &self.dir
    }
}

fn relative_path(path: &Path) -> anyhow::Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        "plugin path must remain relative to its granted directory"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_absolute_and_symlink_escapes() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::fs::write(root.path().join("inside"), "allowed")?;
        std::fs::write(outside.path().join("outside"), "denied")?;
        let scoped = ScopedDirectory::open(root.path())?;
        assert_eq!(scoped.read(Path::new("inside"))?, b"allowed");
        assert!(scoped.read(Path::new("../outside")).is_err());
        assert!(scoped.read(&outside.path().join("outside")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("escape"))?;
            assert!(scoped.read(Path::new("escape/outside")).is_err());
            assert!(scoped
                .write(Path::new("escape/created"), b"denied")
                .is_err());
            assert!(!outside.path().join("created").exists());
        }
        Ok(())
    }
}
