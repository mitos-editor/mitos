use std::path::{Path, PathBuf};

/// States for a file having been changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    /// Not tracked by the VCS.
    Untracked { path: PathBuf },
    /// File has been added to version control.
    Added { path: PathBuf },
    /// File has been modified.
    Modified { path: PathBuf },
    /// File modification is in conflict with a different update.
    Conflict { path: PathBuf },
    /// File has been deleted.
    Deleted { path: PathBuf },
    /// File has been renamed.
    Renamed {
        from_path: PathBuf,
        to_path: PathBuf,
    },
}

impl FileChange {
    pub fn path(&self) -> &Path {
        match self {
            Self::Untracked { path } => path,
            Self::Added { path } => path,
            Self::Modified { path } => path,
            Self::Conflict { path } => path,
            Self::Deleted { path } => path,
            Self::Renamed { to_path, .. } => to_path,
        }
    }
}

/// Independent changes in the staging area and working tree, sharing one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub staged: Option<FileChange>,
    pub unstaged: Option<FileChange>,
}

impl FileStatus {
    /// The destination path, including a working-tree rename after a staged edit.
    pub fn path(&self) -> &Path {
        self.unstaged
            .as_ref()
            .or(self.staged.as_ref())
            .expect("nonempty file status")
            .path()
    }

    /// Combined classification retained for consumers that don't distinguish layers.
    pub fn change(&self) -> &FileChange {
        match (&self.staged, &self.unstaged) {
            (
                Some(staged @ (FileChange::Added { .. } | FileChange::Renamed { .. })),
                Some(FileChange::Modified { .. }),
            ) => staged,
            (_, Some(unstaged)) => unstaged,
            (Some(staged), None) => staged,
            (None, None) => unreachable!("nonempty file status"),
        }
    }
}
