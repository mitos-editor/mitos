//! Editor-owned native file watching, coverage, and polling fallback.
//!
//! Adapted from work by Pascal Kuthe and Blaž Hrastnik in
//! [Helix PR #14544](https://github.com/helix-editor/helix/pull/14544).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::SystemTime;

// Re-export filesentry types (available on all platforms)
pub use filesentry::{CanonicalPathBuf, Event, EventType, Events};
use filesentry::{Filter, ShutdownOnDrop};

mod filter;
use filter::WatchFilter;

use crate::{callbacks::EditorCallbackSender, Editor};
use editor_config::WatchConfig;
use stdx::path::canonicalize_existing;
use tokio::sync::{broadcast, mpsc};

impl Editor {
    /// Deliver one filesystem batch to this editor's protocol clients and features.
    /// Native watches call this through the owning editor's callback destination.
    pub fn handle_file_events(&mut self, events: &Events) {
        use lsp_client::lsp::FileChangeType;
        self.language_servers
            .file_event_handler
            .files_changed(events.iter().filter_map(|event| {
                let ty = match event.ty {
                    EventType::Create => FileChangeType::CREATED,
                    EventType::Modified => FileChangeType::CHANGED,
                    EventType::Delete => FileChangeType::DELETED,
                    EventType::Tempfile => return None,
                };
                Some((event.path.as_std_path().to_owned(), ty))
            }));
        crate::features::auto_reload::handle_file_events(self, events);
        let _ = self.file_watcher.events.send(events.clone());
        self.redraw().request();
    }
}

/// Recursive native watches, with coverage information for the polling fallback.
pub struct Watcher {
    events: broadcast::Sender<Events>,
    callbacks: EditorCallbackSender,
    delivery: event::TaskController,
    generation: Arc<()>,
    watcher: Option<(filesentry::Watcher, ShutdownOnDrop)>,
    filter: Arc<WatchFilter>,
    workspace: PathBuf,
    roots: Vec<PathBuf>,
    active_roots: Vec<(PathBuf, Arc<std::sync::atomic::AtomicBool>)>,
    config: WatchConfig,
    extra_watched_paths: Vec<(PathBuf, Option<SystemTime>)>,
}

impl Watcher {
    pub fn new(config: &WatchConfig, callbacks: EditorCallbackSender) -> Self {
        let (workspace, _) = workspace::find_workspace();
        let mut watcher = Self {
            events: broadcast::channel(64).0,
            callbacks,
            delivery: event::TaskController::new(),
            generation: Arc::new(()),
            watcher: None,
            filter: Arc::new(WatchFilter::new(config, &workspace, [].into_iter())),
            workspace,
            roots: Vec::new(),
            active_roots: Vec::new(),
            config: config.clone(),
            extra_watched_paths: Vec::new(),
        };
        watcher.reload(config);
        watcher
    }

    /// Observe batches delivered to this editor, including while native watches reload.
    /// A lagging consumer must discard its cached filesystem state and read it again.
    pub fn subscribe(&self) -> broadcast::Receiver<Events> {
        self.events.subscribe()
    }

    /// Rebuild watches when configuration or the working directory changes.
    pub fn reload(&mut self, config: &WatchConfig) {
        let (workspace, no_workspace) = workspace::find_workspace();
        let workspace = canonicalize_existing(&workspace);
        if self.config == *config && self.workspace == workspace && self.watcher.is_some() {
            return;
        }
        // Invalidate queued events before replacing or disabling the native watcher.
        self.delivery.cancel();
        self.generation = Arc::new(());
        self.config = config.clone();
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
        // Native threads enqueue without blocking. One forwarding task preserves
        // batch order and waits for frontend capacity instead of dropping LSP changes.
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(event::cancelable_future(
            forward_events(rx, self.callbacks.clone(), Arc::downgrade(&self.generation)),
            self.delivery.restart(),
        ));
        watcher.add_handler(move |events| tx.send(events).is_ok());
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
            self.reload(&self.config.clone());
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
        self.reload(&self.config.clone());
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

async fn forward_events(
    mut events: mpsc::UnboundedReceiver<Events>,
    callbacks: EditorCallbackSender,
    generation: Weak<()>,
) {
    while let Some(events) = events.recv().await {
        let generation = generation.clone();
        callbacks
            .send(move |editor| {
                if generation.ptr_eq(&Arc::downgrade(&editor.file_watcher.generation)) {
                    editor.handle_file_events(&events);
                }
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn native_delivery_waits_for_capacity_and_cancels_blocked_sends() {
        use super::*;
        let (tx, mut callbacks) = mpsc::channel(1);
        let sender = EditorCallbackSender::new(
            event::Redraw::default(),
            move |callback| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(callback).await;
                }
            },
            |_| panic!("native delivery must wait asynchronously for capacity"),
        );
        let (native, events) = mpsc::unbounded_channel();
        let generation = Arc::new(());
        let mut delivery = event::TaskController::new();
        let task = tokio::spawn(event::cancelable_future(
            forward_events(events, sender, Arc::downgrade(&generation)),
            delivery.restart(),
        ));
        for _ in 0..140 {
            native.send(Events::from(Vec::<Event>::new())).unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for _ in 0..140 {
                assert!(callbacks.recv().await.is_some());
            }
        })
        .await
        .unwrap();
        // Retain the native sender while canceling a full frontend destination.
        native.send(Events::from(Vec::<Event>::new())).unwrap();
        native.send(Events::from(Vec::<Event>::new())).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while callbacks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        delivery.cancel();
        assert!(task.await.unwrap().is_none());
        assert!(callbacks.recv().await.is_some());
        assert!(callbacks.recv().await.is_none());
        assert!(native.send(Events::from(Vec::<Event>::new())).is_err());
    }

    #[test]
    fn polling_tracks_missing_metadata_and_preserves_timestamps_when_refreshed() {
        use super::{WatchConfig, Watcher};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEAD");
        let mut watcher = Watcher::new(
            &WatchConfig {
                enable: false,
                ..WatchConfig::default()
            },
            crate::callbacks::EditorCallbackSender::new(
                event::Redraw::default(),
                |_| async {},
                |_| {},
            ),
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
