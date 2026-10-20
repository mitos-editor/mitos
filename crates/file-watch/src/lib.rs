//! Native file watching, ignore filtering, coverage, and metadata polling.
//! Callers supply roots and ignore-file paths, then consume generation-checked batches.
//!
//! Adapted from work by Pascal Kuthe and Blaž Hrastnik in
//! [Helix PR #14544](https://github.com/helix-editor/helix/pull/14544).
pub use filesentry::{CanonicalPathBuf, Event, EventType, Events};
use filesentry::{Filter, ShutdownOnDrop};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::SystemTime,
};
use stdx::path::canonicalize_existing;
use tokio::sync::mpsc;
mod filter;
use filter::WatchFilter;

/// Backend policy. Application configuration and resource discovery stay with callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub enable: bool,
    pub watch_vcs: bool,
    pub require_workspace: bool,
    pub hidden: bool,
    pub ignore: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub max_depth: Option<usize>,
    pub ignore_files: Vec<PathBuf>,
    pub watch_ignore_files: Vec<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enable: true,
            watch_vcs: true,
            require_workspace: true,
            hidden: true,
            ignore: true,
            git_ignore: true,
            git_global: true,
            max_depth: Some(10),
            ignore_files: Vec::new(),
            watch_ignore_files: Vec::new(),
        }
    }
}

/// A native batch retains its generation even while queued by an application.
pub struct EventBatch {
    pub events: Events,
    generation: Weak<()>,
}

impl EventBatch {
    /// Discard obsolete generations before enqueueing application callbacks.
    /// Call `is_current` again when applying a queued callback.
    pub fn is_live(&self) -> bool {
        self.generation.strong_count() != 0
    }

    pub fn is_current(&self, watcher: &Watcher) -> bool {
        self.generation.ptr_eq(&Arc::downgrade(&watcher.generation))
    }
}

/// Recursive native watches, with coverage information for the polling fallback.
pub struct Watcher {
    events: mpsc::UnboundedSender<EventBatch>,
    generation: Arc<()>,
    watcher: Option<(filesentry::Watcher, ShutdownOnDrop)>,
    filter: Arc<WatchFilter>,
    workspace: PathBuf,
    roots: Vec<PathBuf>,
    active_roots: Vec<(PathBuf, Arc<std::sync::atomic::AtomicBool>)>,
    config: Options,
    is_workspace: bool,
    extra_watched_paths: Vec<(PathBuf, Option<SystemTime>)>,
}

impl Watcher {
    /// Whether a native backend is active; individual roots may still be starting.
    pub fn is_running(&self) -> bool {
        self.watcher.is_some()
    }

    pub fn new(
        config: &Options,
        workspace: &Path,
        is_workspace: bool,
    ) -> (Self, mpsc::UnboundedReceiver<EventBatch>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let workspace = workspace.to_owned();
        let mut watcher = Self {
            events,
            generation: Arc::new(()),
            watcher: None,
            filter: Arc::new(WatchFilter::new(config, &workspace, [].into_iter())),
            workspace: workspace.clone(),
            roots: Vec::new(),
            active_roots: Vec::new(),
            config: config.clone(),
            is_workspace,
            extra_watched_paths: Vec::new(),
        };
        watcher.reload(config, &workspace, is_workspace);
        (watcher, receiver)
    }

    /// Rebuild watches when policy or the supplied workspace root changes.
    pub fn reload(&mut self, config: &Options, workspace: &Path, is_workspace: bool) {
        let no_workspace = !is_workspace;
        let workspace = canonicalize_existing(workspace);
        if self.config == *config
            && self.workspace == workspace
            && self.is_workspace == is_workspace
            && self.watcher.is_some()
        {
            return;
        }
        // Invalidate queued events before replacing or disabling the native watcher.
        self.generation = Arc::new(());
        self.config = config.clone();
        self.is_workspace = is_workspace;
        self.watcher = None;
        self.active_roots.clear();
        self.workspace = workspace;
        self.filter = Arc::new(
            WatchFilter::new(
                config,
                &self.workspace,
                self.roots.iter().map(PathBuf::as_path),
            )
            .with_extra_paths(
                self.extra_watched_paths
                    .iter()
                    .map(|(path, _)| path.clone()),
            ),
        );
        let has_metadata = config.watch_vcs && !self.extra_watched_paths.is_empty();
        if !config.enable
            || (config.require_workspace && no_workspace && self.roots.is_empty() && !has_metadata)
        {
            return;
        }
        let watcher = match filesentry::Watcher::new() {
            Ok(watcher) => watcher,
            Err(err) => {
                log::info!("file watcher unavailable; using polling: {err}");
                return;
            }
        };
        watcher.set_filter(self.filter.clone(), false);
        // Native callbacks never block. Consumers validate generation after queueing.
        let events = self.events.clone();
        let generation = Arc::downgrade(&self.generation);
        watcher.add_handler(move |batch| {
            events
                .send(EventBatch {
                    events: batch,
                    generation: generation.clone(),
                })
                .is_ok()
        });
        if !config.require_workspace || !no_workspace {
            self.watch_root(&watcher, self.workspace.clone());
        }
        for root in self.roots.clone() {
            self.watch_root(&watcher, root);
        }
        if config.watch_vcs {
            for (path, _) in self.extra_watched_paths.clone() {
                // Watch an existing ancestor so index creation and atomic replacement work too.
                let Some(root) = path
                    .parent()
                    .and_then(|parent| parent.ancestors().find(|path| path.is_dir()))
                else {
                    continue;
                };
                if !self
                    .active_roots
                    .iter()
                    .any(|(watched, _)| root.starts_with(watched))
                {
                    self.watch_root(&watcher, root.to_owned());
                }
            }
        }
        let shutdown = watcher.shutdown_guard();
        watcher.start();
        self.watcher = Some((watcher, shutdown));
    }

    fn watch_root(&mut self, watcher: &filesentry::Watcher, root: PathBuf) {
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready_ = ready.clone();
        match watcher.add_root(&root, true, move |ok| {
            ready_.store(ok, std::sync::atomic::Ordering::Release)
        }) {
            Ok(_) => self.active_roots.push((root, ready)),
            Err(err) => log::warn!("failed to watch {}: {err}", root.display()),
        }
    }

    /// True only after a native root is ready and the path passes its filter.
    /// Deleted and not-yet-created paths are checked lexically as well.
    pub fn is_watching(&self, path: &Path) -> bool {
        let path = canonicalize_existing(path);
        self.active_roots.iter().any(|(root, ready)| {
            ready.load(std::sync::atomic::Ordering::Acquire) && path.starts_with(root)
        }) && !self.filter.ignore_path_rec(&path, Some(path.is_dir()))
    }

    /// Invalidate cached ignore files and recrawl affected watch roots.
    pub fn refresh_filter(&mut self) {
        self.filter = Arc::new(
            WatchFilter::new(
                &self.config,
                &self.workspace,
                self.roots.iter().map(PathBuf::as_path),
            )
            .with_extra_paths(
                self.extra_watched_paths
                    .iter()
                    .map(|(path, _)| path.clone()),
            ),
        );
        if let Some((watcher, _)) = &self.watcher {
            watcher.set_filter(self.filter.clone(), true);
        }
    }

    /// Add an explicit LSP root even when the working directory is not a workspace.
    pub fn add_root(&mut self, root: &Path) {
        let Ok(root) = root.canonicalize() else {
            return;
        };
        if !root.is_dir() || self.roots.contains(&root) {
            return;
        }
        self.roots.push(root.clone());
        self.refresh_filter();
        if let Some((watcher, _)) = &self.watcher {
            let watcher = watcher.clone();
            self.watch_root(&watcher, root);
        } else {
            self.reload(
                &self.config.clone(),
                &self.workspace.clone(),
                self.is_workspace,
            );
        }
    }

    /// Watch explicit VCS metadata, retaining polling when native watching is unavailable.
    /// Preserve timestamps when the set is refreshed so changes are not swallowed.
    pub fn set_extra_watched_paths(&mut self, mut paths: Vec<PathBuf>) {
        for path in &mut paths {
            // Keep the native representation, including verbatim Windows prefixes.
            *path = canonicalize_existing(path);
        }
        paths.sort();
        paths.dedup();
        if self
            .extra_watched_paths
            .iter()
            .map(|(path, _)| path)
            .eq(paths.iter())
        {
            return;
        }
        let old = std::mem::take(&mut self.extra_watched_paths);
        self.extra_watched_paths = paths
            .into_iter()
            .map(|path| {
                let mtime = old
                    .iter()
                    .find(|(old, _)| *old == path)
                    .map(|(_, time)| *time)
                    .unwrap_or_else(|| path.metadata().ok().and_then(|m| m.modified().ok()));
                (path, mtime)
            })
            .collect();
        // Native roots cannot be removed individually. Rebuild when their demand changes.
        self.watcher = None;
        self.reload(
            &self.config.clone(),
            &self.workspace.clone(),
            self.is_workspace,
        );
    }

    pub fn poll_extra_paths(&mut self) -> bool {
        let mut changed = false;
        for (path, previous) in &mut self.extra_watched_paths {
            let current = path.metadata().ok().and_then(|m| m.modified().ok());
            changed |= current != *previous;
            *previous = current;
        }
        changed
    }

    pub fn is_vcs_path(&self, path: &Path) -> bool {
        self.extra_watched_paths
            .iter()
            .any(|(watched, _)| watched == path)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn queued_batches_are_bound_to_their_watcher_and_configuration() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let options = Options {
            enable: false,
            ..Options::default()
        };
        let (mut first, _) = Watcher::new(&options, dir.path(), true);
        let (other, _) = Watcher::new(&options, dir.path(), true);
        let batch = EventBatch {
            events: Events::from(Vec::<Event>::new()),
            generation: Arc::downgrade(&first.generation),
        };
        assert!(batch.is_current(&first));
        assert!(batch.is_live());
        assert!(!batch.is_current(&other));
        first.reload(&options, dir.path(), true);
        assert!(!batch.is_current(&first));
        assert!(!batch.is_live());
    }

    #[test]
    fn polling_tracks_missing_metadata_and_preserves_timestamps_when_refreshed() {
        use super::{Options, Watcher};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEAD");
        let (mut watcher, _) = Watcher::new(
            &Options {
                enable: false,
                ..Options::default()
            },
            dir.path(),
            true,
        );
        watcher.set_extra_watched_paths(vec![path.clone()]);
        let native_path = dir.path().canonicalize().unwrap().join("HEAD");
        assert!(watcher.is_vcs_path(&native_path));
        assert!(!watcher.poll_extra_paths());
        std::fs::write(&path, "ref: refs/heads/main\n").unwrap();
        watcher.set_extra_watched_paths(vec![path.clone(), path.clone()]);
        assert!(watcher.is_vcs_path(&path.canonicalize().unwrap()));
        assert!(watcher.poll_extra_paths());
        assert!(!watcher.poll_extra_paths());
        std::fs::remove_file(path).unwrap();
        assert!(watcher.poll_extra_paths());
    }
}
