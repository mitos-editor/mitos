//! Workspace invalidation shared by query caches and frontend result providers.

use crate::{
    callbacks::EditorCallbackSender,
    file_watcher::{EventType, Events},
    DocumentId, Editor,
};
use std::path::{Path, PathBuf};
use tokio::{sync::broadcast, time::Instant};

#[derive(Clone, Copy, Default)]
pub enum Documents {
    #[default]
    None,
    Workspace,
    All,
}

#[derive(Clone, Copy, Default)]
pub enum Polling {
    /// Rescan even when the root has a native watch (coverage may exclude entries).
    #[default]
    Always,
    Unwatched,
}

#[derive(Default)]
pub struct Policy {
    pub contents: bool,
    pub vcs: bool,
    pub ignore_files: bool,
    pub documents: Documents,
    pub polling: Polling,
}

type Versions = Vec<(DocumentId, i32, Option<PathBuf>)>;

pub struct WorkspaceChanges {
    owner: EditorCallbackSender,
    root: PathBuf,
    pub policy: Policy,
    events: broadcast::Receiver<Events>,
    versions: Versions,
    checked: Instant,
}

fn document_versions(editor: &Editor, root: &Path, scope: Documents) -> Versions {
    let mut versions: Vec<_> = editor
        .documents()
        .filter_map(|doc| {
            let path = doc.path().map(stdx::path::canonicalize_existing);
            if !matches!(scope, Documents::All)
                && !path.as_ref().is_some_and(|path| path.starts_with(root))
            {
                return None;
            }
            Some((doc.id(), doc.version(), path))
        })
        .collect();
    versions.sort_unstable_by_key(|(id, _, _)| *id);
    versions
}

pub fn is_ignore_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == ".gitignore" || name == ".ignore" || name == "filesentryignore")
        || path.ends_with(".mitos/ignore")
}

impl WorkspaceChanges {
    pub fn new(editor: &Editor, root: &Path, policy: Policy) -> Self {
        let root = stdx::path::canonicalize_existing(root);
        let versions = document_versions(editor, &root, policy.documents);
        Self {
            owner: editor.callback_sender().clone(),
            root,
            policy,
            versions,
            events: editor.file_watcher.subscribe(),
            checked: Instant::now(),
        }
    }

    /// Sample events and document revisions. A lost event batch conservatively invalidates.
    /// This does not schedule work or acknowledge that a consumer has refreshed.
    pub fn changed(&mut self, editor: &Editor, root: &Path) -> bool {
        if !self.owner.same_destination(editor.callback_sender()) {
            return false;
        }
        let root = stdx::path::canonicalize_existing(root);
        let mut changed = root != self.root;
        self.root = root;
        if !matches!(self.policy.documents, Documents::None) {
            let versions = document_versions(editor, &self.root, self.policy.documents);
            changed |= versions != self.versions;
            self.versions = versions;
        }
        loop {
            match self.events.try_recv() {
                Ok(events) => {
                    changed |= events.iter().any(|event| {
                        let path = event.path.as_std_path();
                        event.ty != EventType::Tempfile
                            && ((self.policy.ignore_files && is_ignore_file(path))
                                || (self.policy.vcs && editor.file_watcher.is_vcs_path(path))
                                || ((self.policy.contents
                                    || event.ty != EventType::Modified
                                    || path.is_dir())
                                    && (path.starts_with(&self.root)
                                        || self.root.starts_with(path))))
                    })
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => changed = true,
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.events = editor.file_watcher.subscribe();
                    changed = true;
                    break;
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
            }
        }
        changed
    }

    pub fn poll_due(&self, editor: &Editor) -> bool {
        if !self.owner.same_destination(editor.callback_sender()) {
            return false;
        }
        let poll = &editor.config().files.reload.poll;
        poll.enable
            && (matches!(self.policy.polling, Polling::Always)
                || !editor.file_watcher.is_watching(&self.root))
            && self.checked.elapsed() >= std::time::Duration::from_millis(poll.interval.max(100))
    }

    /// Call when a scan starts/completes or a cache is invalidated, retaining caller timing.
    pub fn mark_refreshed(&mut self) {
        self.checked = Instant::now();
    }
}
