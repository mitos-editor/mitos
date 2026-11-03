//! Workspace root discovery and execution trust policy.
//!
//! Callers supply the trust-store directory; this crate does not discover
//! application resources, configuration paths, or editor state.
pub mod trust;
pub use trust::WorkspaceTrust;

use std::path::{Path, PathBuf};
use stdx::env::current_working_dir;

/// A resolved workspace root, or the requested directory when no marker exists.
/// Resolve again after changing directories or when workspace markers may have changed.
#[derive(Debug, Clone)]
pub struct ResolvedWorkspace {
    root: PathBuf,
    is_workspace: bool,
}

impl ResolvedWorkspace {
    pub fn current() -> Self {
        Self::resolve(current_working_dir())
    }

    pub fn resolve(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let root = dir.ancestors().find(|ancestor| {
            [".git", ".svn", ".jj", ".mitos"]
                .iter()
                .any(|marker| ancestor.join(marker).exists())
        });
        Self {
            root: root.unwrap_or(dir).to_owned(),
            is_workspace: root.is_some(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_workspace(&self) -> bool {
        self.is_workspace
    }
}

/// Find the current workspace, retaining the legacy fallback flag for callers.
pub fn find_workspace() -> (PathBuf, bool) {
    let workspace = ResolvedWorkspace::current();
    (workspace.root, !workspace.is_workspace)
}

pub fn find_workspace_in(dir: impl AsRef<Path>) -> (PathBuf, bool) {
    let workspace = ResolvedWorkspace::resolve(dir);
    (workspace.root, !workspace.is_workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_records_markers_and_refreshes_after_they_change() {
        let directory = tempfile::tempdir().unwrap();
        let child = directory.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let before = ResolvedWorkspace::resolve(&child);
        std::fs::create_dir(directory.path().join(".mitos")).unwrap();
        let marked = ResolvedWorkspace::resolve(&child);
        assert_eq!(marked.root(), directory.path());
        assert!(marked.is_workspace());
        assert_ne!(before.root(), marked.root());
        std::fs::remove_dir(directory.path().join(".mitos")).unwrap();
        let after = ResolvedWorkspace::resolve(&child);
        assert_eq!(after.root(), before.root());
        assert_eq!(after.is_workspace(), before.is_workspace());
    }
}
