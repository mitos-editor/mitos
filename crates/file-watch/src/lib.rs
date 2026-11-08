//! Native file watching, ignore filtering, coverage, and metadata polling.
//! Callers supply roots and ignore-file paths, then consume generation-checked batches.
//!
//! Adapted from work by Pascal Kuthe and Blaž Hrastnik in
//! [Helix PR #14544](https://github.com/helix-editor/helix/pull/14544).
pub use filesentry::{CanonicalPathBuf, Event, EventType, Events};
use filesentry::{Filter, ShutdownOnDrop};
use std::{
    collections::BTreeMap,
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

/// Paths requested by one owner or the union of several owners.
/// Metadata watches exclude siblings; individual files preserve source coverage.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Demand {
    pub roots: Vec<PathBuf>,
    pub metadata: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

impl Demand {
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.metadata.is_empty() && self.files.is_empty()
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
    extra_watched_paths: BTreeMap<PathBuf, Option<SystemTime>>,
    extra_watched_files: Vec<PathBuf>,
}

/// Keep metadata symlinks available for polling alongside their native-watched targets.
/// Resolve the parent directory while retaining the configured leaf spelling.
fn metadata_paths(path: &Path) -> [PathBuf; 2] {
    let target = canonicalize_existing(path);
    let alias = path
        .parent()
        .zip(path.file_name())
        .map(|(parent, name)| canonicalize_existing(parent).join(name))
        .unwrap_or_else(|| target.clone());
    [alias, target]
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
            extra_watched_paths: BTreeMap::new(),
            extra_watched_files: Vec::new(),
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
        self.filter = self.build_filter();
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

    fn build_filter(&self) -> Arc<WatchFilter> {
        Arc::new(
            WatchFilter::new(
                &self.config,
                &self.workspace,
                self.roots.iter().map(PathBuf::as_path),
            )
            .with_extra_paths(
                self.extra_watched_paths
                    .iter()
                    .filter(|(path, _)| !self.extra_watched_files.contains(path))
                    .map(|(path, _)| path.clone()),
            )
            .with_extra_files(self.extra_watched_files.iter().cloned()),
        )
    }

    /// Invalidate cached ignore files and recrawl affected watch roots.
    pub fn refresh_filter(&mut self) {
        self.filter = self.build_filter();
        if let Some((watcher, _)) = &self.watcher {
            watcher.set_filter(self.filter.clone(), true);
        }
    }

    /// Project requested paths, releasing roots no longer demanded by any owner.
    /// Unchanged unions keep the native watcher and queued generation alive.
    pub fn set_paths(&mut self, demand: Demand) {
        let mut roots: Vec<_> = demand
            .roots
            .into_iter()
            .filter_map(|root| root.canonicalize().ok())
            .filter(|root| root.is_dir())
            .collect();
        roots.sort();
        roots.dedup();
        let mut files: Vec<_> = demand
            .files
            .into_iter()
            .map(|file| canonicalize_existing(&file))
            .collect();
        files.sort();
        files.dedup();
        let mut paths: Vec<_> = demand
            .metadata
            .into_iter()
            .flat_map(|path| metadata_paths(&path))
            .chain(files.iter().cloned())
            .collect();
        paths.sort();
        paths.dedup();
        let same_metadata =
            self.extra_watched_files == files && self.extra_watched_paths.keys().eq(paths.iter());
        if self.roots == roots && same_metadata {
            return;
        }
        if same_metadata
            && self.watcher.is_some()
            && self
                .roots
                .iter()
                .all(|root| roots.binary_search(root).is_ok())
        {
            let added: Vec<_> = roots
                .iter()
                .filter(|root| !self.roots.contains(root))
                .cloned()
                .collect();
            self.roots = roots;
            self.refresh_filter();
            let watcher = self.watcher.as_ref().unwrap().0.clone();
            for root in added {
                self.watch_root(&watcher, root);
            }
            return;
        }
        self.roots = roots;
        self.extra_watched_files = files;
        let mut old = std::mem::take(&mut self.extra_watched_paths);
        self.extra_watched_paths = paths
            .into_iter()
            .map(|path| {
                let mtime = old.remove(&path).unwrap_or_else(|| {
                    path.symlink_metadata().ok().and_then(|m| m.modified().ok())
                });
                (path, mtime)
            })
            .collect();
        // The native backend has no unwatch operation. Rebuild the changed union.
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
            // The native backend omits symlinks; detect link replacement in metadata/focus polls.
            let current = path.symlink_metadata().ok().and_then(|m| m.modified().ok());
            changed |= current != *previous;
            *previous = current;
        }
        changed
    }

    pub fn is_vcs_path(&self, path: &Path) -> bool {
        self.extra_watched_paths.contains_key(path)
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
        watcher.set_paths(super::Demand {
            metadata: vec![path.clone()],
            ..Default::default()
        });
        let native_path = dir.path().canonicalize().unwrap().join("HEAD");
        assert!(watcher.is_vcs_path(&native_path));
        assert!(!watcher.poll_extra_paths());
        let batch = super::EventBatch {
            events: super::Events::from(Vec::<super::Event>::new()),
            generation: std::sync::Arc::downgrade(&watcher.generation),
        };
        std::fs::write(&path, "ref: refs/heads/main\n").unwrap();
        watcher.set_paths(super::Demand {
            metadata: vec![path.clone(), path.clone()],
            ..Default::default()
        });
        assert!(watcher.is_vcs_path(&path.canonicalize().unwrap()));
        assert!(batch.is_current(&watcher)); // Identical demand unions retain queued events.
        assert!(watcher.poll_extra_paths());
        assert!(!watcher.poll_extra_paths());
        std::fs::remove_file(&path).unwrap();
        assert!(watcher.poll_extra_paths());
        watcher.set_paths(super::Demand::default());
        assert!(!watcher.is_vcs_path(&path));
        assert!(!watcher.poll_extra_paths());
        assert!(!batch.is_current(&watcher));
    }
}

#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn external_metadata_links_watch_targets_and_poll_retargeting() {
        let workspace = tempfile::tempdir().unwrap();
        let links = tempfile::tempdir().unwrap();
        let targets = tempfile::tempdir().unwrap();
        let target = targets.path().canonicalize().unwrap().join("first");
        let next_target = targets.path().canonicalize().unwrap().join("second");
        let alias = links.path().canonicalize().unwrap().join("attributes");
        std::fs::write(&target, "first\n").unwrap();
        std::fs::write(&next_target, "second\n").unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let (mut watcher, mut receiver) = Watcher::new(&Options::default(), workspace.path(), true);
        let demand = || Demand {
            metadata: vec![alias.clone()],
            ..Default::default()
        };
        watcher.set_paths(demand());
        assert!(watcher.is_vcs_path(&alias));
        assert!(watcher.is_vcs_path(&target));
        fn ready(watcher: &Watcher, path: &Path) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !watcher.is_watching(path) {
                assert!(
                    Instant::now() < deadline,
                    "metadata watch did not become ready: {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn event(
            receiver: &mut mpsc::UnboundedReceiver<EventBatch>,
            watcher: &Watcher,
            path: &Path,
        ) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(batch) = receiver.try_recv()
                    && batch.is_current(watcher)
                    && batch
                        .events
                        .iter()
                        .any(|event| event.path.as_std_path() == path && watcher.is_vcs_path(path))
                {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "missing VCS metadata event: {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        ready(&watcher, &target);
        std::fs::write(&target, "updated first\n").unwrap();
        event(&mut receiver, &watcher, &target);
        assert!(watcher.poll_extra_paths());
        assert!(!watcher.poll_extra_paths());
        std::fs::OpenOptions::new()
            .write(true)
            .open(&next_target)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(target.metadata().unwrap().modified().unwrap()),
            )
            .unwrap();
        let replacement = alias.with_file_name("replacement");
        std::os::unix::fs::symlink(&next_target, &replacement).unwrap();
        std::fs::rename(replacement, &alias).unwrap();
        // filesentry excludes symlink nodes. The regular metadata/focus poll detects
        // replacement of the link itself, even if the two targets share an mtime.
        assert!(watcher.poll_extra_paths());
        assert!(!watcher.poll_extra_paths());
        // The application refreshes demand after invalidation and follows the new target.
        watcher.set_paths(demand());
        assert!(watcher.is_vcs_path(&next_target));
        assert!(!watcher.is_vcs_path(&target));
        ready(&watcher, &next_target);
        std::fs::write(&next_target, "updated second\n").unwrap();
        event(&mut receiver, &watcher, &next_target);
    }
}
