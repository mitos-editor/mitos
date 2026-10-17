//! Workspace root discovery and execution trust policy.
//!
//! Callers supply the trust-store directory; this crate does not discover
//! application resources, configuration paths, or editor state.
pub mod trust;
pub use trust::WorkspaceTrust;

use std::path::{Path, PathBuf};
use stdx::env::current_working_dir;

/// Finds the current workspace folder.
/// Used as a ceiling dir for LSP root resolution, the filepicker and potentially as a future filewatching root
///
/// This function starts searching the FS upward from the CWD
/// and returns the first directory that contains either `.git`, `.svn`, `.jj` or `.mitos`.
/// If no workspace was found returns (CWD, true).
/// Otherwise (workspace, false) is returned
pub fn find_workspace() -> (PathBuf, bool) {
    let current_dir = current_working_dir();
    find_workspace_in(current_dir)
}

pub fn find_workspace_in(dir: impl AsRef<Path>) -> (PathBuf, bool) {
    let dir = dir.as_ref();
    for ancestor in dir.ancestors() {
        if ancestor.join(".git").exists()
            || ancestor.join(".svn").exists()
            || ancestor.join(".jj").exists()
            || ancestor.join(".mitos").exists()
        {
            return (ancestor.to_owned(), false);
        }
    }

    (dir.to_owned(), true)
}
