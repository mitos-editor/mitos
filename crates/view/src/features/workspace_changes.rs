//! Workspace invalidation shared by query caches and frontend result providers.

use crate::{
    callbacks::EditorCallbackSender,
    file_watcher::{EventType, Events},
    DocumentId, Editor,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
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
    source_root: PathBuf,
    paths: HashMap<DocumentId, (PathBuf, PathBuf)>,
    resolved_at: Instant,
    pub policy: Policy,
    events: broadcast::Receiver<Events>,
    versions: Versions,
    checked: Instant,
}

fn absolute_root(root: &Path) -> PathBuf {
    if root.is_absolute() {
        root.to_owned()
    } else {
        stdx::env::current_working_dir().join(root)
    }
}

fn document_versions(
    editor: &Editor,
    root: &Path,
    scope: Documents,
    paths: &mut HashMap<DocumentId, (PathBuf, PathBuf)>,
) -> Versions {
    if matches!(scope, Documents::None) {
        paths.clear();
        return Vec::new();
    }
    paths.retain(|id, _| editor.document(*id).is_some());
    let mut versions: Vec<_> = editor
        .documents()
        .filter_map(|doc| {
            let path = doc.path().map(|path| {
                let entry = paths
                    .entry(doc.id())
                    .or_insert_with(|| (path.to_owned(), stdx::path::canonicalize_existing(path)));
                if entry.0 != path {
                    *entry = (path.to_owned(), stdx::path::canonicalize_existing(path));
                }
                entry.1.clone()
            });
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
    path.file_name().is_some_and(|name| {
        name == ".gitignore"
            || name == ".ignore"
            || name == "filesentryignore"
            || name == ".filesentryignore"
    }) || path.ends_with(".mitos/ignore")
        || path.ends_with(".git/info/exclude")
        || (path.file_name().is_some_and(|name| name == "ignore")
            && path == stdx::path::canonicalize_existing(&loader::config_dir().join("ignore")))
}

impl WorkspaceChanges {
    pub fn new(editor: &Editor, root: &Path, policy: Policy) -> Self {
        let source_root = absolute_root(root);
        let root = stdx::path::canonicalize_existing(root);
        let mut paths = HashMap::new();
        let versions = document_versions(editor, &root, policy.documents, &mut paths);
        Self {
            owner: editor.callback_sender().clone(),
            root,
            source_root,
            paths,
            resolved_at: Instant::now(),
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
        let mut changed = false;
        // Resolve identities after filesystem activity, or periodically when events
        // are unavailable. Ordinary redraws only compare document metadata.
        let poll = &editor.config().files.reload.poll;
        let source_root = absolute_root(root);
        let mut resolve = source_root != self.source_root
            || (poll.enable
                && self.resolved_at.elapsed()
                    >= std::time::Duration::from_millis(poll.interval.max(100)));
        loop {
            match self.events.try_recv() {
                Ok(events) => {
                    resolve = true;
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
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    changed = true;
                    resolve = true;
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.events = editor.file_watcher.subscribe();
                    changed = true;
                    resolve = true;
                    break;
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
            }
        }
        if resolve {
            let resolved = stdx::path::canonicalize_existing(root);
            changed |= resolved != self.root;
            self.root = resolved;
            self.source_root = source_root;
            self.paths.clear();
            self.resolved_at = Instant::now();
        }
        let versions =
            document_versions(editor, &self.root, self.policy.documents, &mut self.paths);
        changed |= versions != self.versions;
        self.versions = versions;
        changed
    }

    /// A consumer has refreshed known, synchronous edits to editor documents.
    /// Keep filesystem events and polling due so independent changes are still observed.
    pub fn mark_documents_refreshed(&mut self, editor: &Editor) {
        self.versions =
            document_versions(editor, &self.root, self.policy.documents, &mut self.paths);
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
