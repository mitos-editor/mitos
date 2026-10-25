//! Compact labels for the open document set, without changing document identity.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    path::Path,
};

use crate::{DocumentId, Editor};

impl Editor {
    /// Prefer basenames. Ambiguous filenames use workspace-relative paths; labels
    /// that still collide across workspaces use the full source path. Snapshot
    /// suffixes are stable and are assigned when each independent review opens.
    pub fn document_tab_labels(&self) -> HashMap<DocumentId, String> {
        let mut paths: HashMap<&OsStr, HashSet<&Path>> = HashMap::new();
        for doc in self.documents() {
            if let Some(path) = doc.display_metadata().source_path
                && let Some(filename) = path.file_name()
            {
                paths.entry(filename).or_default().insert(path);
            }
        }
        let mut labels = HashMap::new();
        let mut candidates: HashMap<String, HashSet<&Path>> = HashMap::new();
        for doc in self.documents() {
            let display = doc.display_metadata();
            let label = if let Some(source_path) = display.source_path {
                let short = source_path
                    .file_name()
                    .map(Path::new)
                    .unwrap_or(source_path);
                let path = if paths
                    .get(short.as_os_str())
                    .is_some_and(|paths| paths.len() > 1)
                {
                    source_path
                        .strip_prefix(doc.workspace_root())
                        .unwrap_or(source_path)
                } else {
                    short
                };
                let label = display.format_name(path);
                candidates
                    .entry(label.clone())
                    .or_default()
                    .insert(source_path);
                label
            } else {
                doc.display_name().into_owned()
            };
            labels.insert(doc.id(), label);
        }
        for doc in self.documents() {
            let display = doc.display_metadata();
            let label = labels.get_mut(&doc.id()).unwrap();
            if candidates.get(label).is_some_and(|paths| paths.len() > 1)
                && let Some(path) = display.source_path
            {
                *label = display.format_name(path);
            }
        }
        labels
    }
}
