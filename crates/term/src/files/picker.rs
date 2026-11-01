//! A file picker's discovery cache. Only blocking workers hold this lock.
use editor_config::FilePickerConfig;
use search::FileIndex;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{sync::broadcast, time::Instant};
use view::{
    features::workspace_changes::is_ignore_file,
    file_watcher::{EventType, Events},
    Editor,
};

#[derive(Debug)]
pub(crate) struct FilePickerCache {
    events: broadcast::Receiver<Events>,
    source: Option<(PathBuf, FilePickerConfig)>,
    index: Option<FileIndex>,
    scanned: Instant,
    #[cfg(test)]
    pub(crate) scans: usize,
    #[cfg(test)]
    pub(crate) updates: usize,
}
impl FilePickerCache {
    pub(crate) fn new(editor: &Editor) -> Self {
        Self {
            events: editor.file_watcher.subscribe(),
            source: None,
            index: None,
            scanned: Instant::now(),
            #[cfg(test)]
            scans: 0,
            #[cfg(test)]
            updates: 0,
        }
    }
    // Drain before scanning, never afterward: events arriving during a scan must
    // remain queued for the next refresh, even if they concern an earlier directory.
    fn changes(&mut self, root: &Path) -> (bool, Vec<PathBuf>) {
        let root = stdx::path::canonicalize_existing(root);
        let mut full = false;
        let mut paths = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(events) => {
                    for event in events.iter() {
                        if event.ty == EventType::Tempfile {
                            continue;
                        }
                        let path = event.path.as_std_path();
                        if is_ignore_file(path) {
                            full = true;
                        } else if path.starts_with(&root) || root.starts_with(path) {
                            if event.ty == EventType::Modified {
                                full |= !path
                                    .symlink_metadata()
                                    .is_ok_and(|metadata| metadata.is_file());
                            } else if event.ty == EventType::Delete
                                && path.symlink_metadata().is_ok()
                            {
                                // Atomic replacements and case-only renames can
                                // make a deleted name refer to a new/existing file.
                                full = true;
                            } else if paths.len() < 128 {
                                paths.push(path.to_owned());
                            } else {
                                full = true;
                            }
                        } else {
                            // External activity may change a custom ignore rule
                            // or alias target whose effect is unknown here.
                            full = true;
                        }
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => full = true,
                Err(broadcast::error::TryRecvError::Closed) => {
                    full = true;
                    break;
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
            }
        }
        (full, paths)
    }
    pub(crate) fn initial(
        &mut self,
        root: &Path,
        config: &FilePickerConfig,
        cancelled: impl Fn() -> bool,
        emit: impl FnMut(&Path) -> bool,
    ) {
        self.index = None;
        self.changes(root);
        if let Some(index) = FileIndex::scan(
            root,
            &super::picker_options(config),
            super::excluded_types(),
            cancelled,
            emit,
        ) {
            self.source = Some((root.to_owned(), config.clone()));
            self.index = Some(index);
            self.scanned = Instant::now();
            #[cfg(test)]
            {
                self.scans += 1;
            }
        }
    }
    pub(crate) fn refresh(
        &mut self,
        root: &Path,
        config: &FilePickerConfig,
        poll_interval: Option<Duration>,
        cancelled: impl Fn() -> bool,
    ) -> Vec<PathBuf> {
        // Taking the baseline first means cancellation after consuming events can
        // never leave an apparently valid index that missed those events.
        let previous = self.index.take();
        let (full, paths) = self.changes(root);
        let same_source = self
            .source
            .as_ref()
            .is_some_and(|(path, old)| path == root && old == config);
        let poll_due = poll_interval.is_some_and(|interval| self.scanned.elapsed() >= interval);
        let candidate = if !full && same_source && !poll_due {
            previous.and_then(|index| index.update(&paths, &cancelled))
        } else {
            drop(previous);
            None
        };
        let index = if let Some(index) = candidate {
            #[cfg(test)]
            {
                self.updates += 1;
            }
            index
        } else {
            if cancelled() {
                return Vec::new();
            }
            let Some(index) = FileIndex::scan(
                root,
                &super::picker_options(config),
                super::excluded_types(),
                &cancelled,
                |_| true,
            ) else {
                return Vec::new();
            };
            self.scanned = Instant::now();
            #[cfg(test)]
            {
                self.scans += 1;
            }
            index
        };
        let rows = index
            .paths()
            .take_while(|_| !cancelled())
            .cloned()
            .collect();
        if cancelled() {
            return Vec::new();
        }
        self.source = Some((root.to_owned(), config.clone()));
        self.index = Some(index);
        rows
    }
}

#[cfg(all(test, feature = "integration"))]
mod tests {
    use super::*;
    use crate::ui::picker::test::application;
    use std::cell::Cell;
    use view::file_watcher::{CanonicalPathBuf, Event};
    fn changed(editor: &mut Editor, path: &Path, ty: EventType) {
        editor.handle_file_events(&Events::from(vec![Event {
            path: CanonicalPathBuf::assert_canonicalized(&stdx::path::canonicalize_existing(path)),
            ty,
        }]));
    }
    fn config() -> FilePickerConfig {
        FilePickerConfig {
            parents: false,
            git_global: false,
            ..FilePickerConfig::default()
        }
    }
    #[tokio::test]
    async fn ordinary_changes_are_incremental_and_polling_config_and_directories_rescan(
    ) -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::write(root.join("old.rs"), "")?;
        let mut cache = FilePickerCache::new(&app.editor);
        let mut config = config();
        cache.initial(&root, &config, || false, |_| true);
        std::fs::write(root.join("new.rs"), "")?;
        changed(&mut app.editor, &root.join("new.rs"), EventType::Create);
        assert_eq!(cache.refresh(&root, &config, None, || false).len(), 2);
        assert_eq!((cache.scans, cache.updates), (1, 1));
        std::fs::remove_file(root.join("old.rs"))?;
        changed(&mut app.editor, &root.join("old.rs"), EventType::Delete);
        assert_eq!(
            cache.refresh(&root, &config, None, || false),
            [root.join("new.rs")]
        );
        assert_eq!((cache.scans, cache.updates), (1, 2));
        // No event refreshes are manual/polling/resume fallbacks.
        cache.refresh(&root, &config, None, || false);
        assert_eq!(cache.scans, 2);
        std::fs::write(root.join(".hidden"), "")?;
        config.hidden = false;
        changed(&mut app.editor, &root.join("new.rs"), EventType::Create);
        assert_eq!(cache.refresh(&root, &config, None, || false).len(), 2);
        assert_eq!(cache.scans, 3);
        changed(&mut app.editor, &root.join("new.rs"), EventType::Create);
        cache.refresh(&root, &config, Some(Duration::ZERO), || false);
        assert_eq!(cache.scans, 4);
        std::fs::create_dir(root.join("directory"))?;
        std::fs::write(root.join("directory/nested"), "")?;
        changed(&mut app.editor, &root.join("directory"), EventType::Create);
        assert_eq!(cache.refresh(&root, &config, None, || false).len(), 3);
        std::fs::remove_dir_all(root.join("directory"))?;
        changed(&mut app.editor, &root.join("directory"), EventType::Delete);
        assert_eq!(cache.refresh(&root, &config, None, || false).len(), 2);
        assert_eq!(cache.scans, 6);
        Ok(())
    }
    #[tokio::test]
    async fn repository_marker_creation_reloads_cached_gitignore_rules() -> anyhow::Result<()> {
        for marker in [".git", ".jj"] {
            let mut app = application(false)?;
            let dir = tempfile::tempdir()?;
            let root = dir.path().canonicalize()?;
            std::fs::write(root.join(".gitignore"), "ignored.rs\n")?;
            std::fs::write(root.join("ignored.rs"), "")?;
            let mut cache = FilePickerCache::new(&app.editor);
            let config = config();
            cache.initial(&root, &config, || false, |_| true);
            changed(&mut app.editor, &root.join("ignored.rs"), EventType::Create);
            assert_eq!(cache.refresh(&root, &config, None, || false).len(), 1);
            assert_eq!(cache.updates, 1);
            std::fs::write(root.join(marker), "")?;
            changed(&mut app.editor, &root.join(marker), EventType::Create);
            assert!(cache.refresh(&root, &config, None, || false).is_empty());
            assert_eq!(cache.scans, 2);
        }
        Ok(())
    }
    #[tokio::test]
    async fn ignore_changes_and_lost_event_batches_force_full_scans() -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::write(root.join("old.rs"), "")?;
        let mut cache = FilePickerCache::new(&app.editor);
        let config = config();
        cache.initial(&root, &config, || false, |_| true);
        // Warm the incremental matcher, then change the rules it cached.
        changed(&mut app.editor, &root.join("old.rs"), EventType::Create);
        cache.refresh(&root, &config, None, || false);
        std::fs::write(root.join(".ignore"), "*.rs\n")?;
        changed(&mut app.editor, &root.join(".ignore"), EventType::Modified);
        assert!(cache.refresh(&root, &config, None, || false).is_empty());
        assert_eq!(cache.scans, 2);
        for _ in 0..100 {
            changed(&mut app.editor, &root.join("old.rs"), EventType::Create);
        }
        cache.refresh(&root, &config, None, || false);
        assert_eq!(cache.scans, 3);
        Ok(())
    }
    #[tokio::test]
    async fn ambiguous_external_and_missing_directory_events_rescan() -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::create_dir(root.join("directory"))?;
        std::fs::write(root.join("directory/old"), "")?;
        let mut cache = FilePickerCache::new(&app.editor);
        let config = config();
        cache.initial(&root, &config, || false, |_| true);
        std::fs::remove_dir_all(root.join("directory"))?;
        std::fs::write(root.join("new"), "")?;
        changed(
            &mut app.editor,
            &root.join("directory"),
            EventType::Modified,
        );
        changed(&mut app.editor, &root.join("new"), EventType::Create);
        assert_eq!(
            cache.refresh(&root, &config, None, || false),
            [root.join("new")]
        );
        let outside = tempfile::NamedTempFile::new()?;
        changed(&mut app.editor, outside.path(), EventType::Modified);
        changed(&mut app.editor, &root.join("new"), EventType::Create);
        cache.refresh(&root, &config, None, || false);
        assert_eq!((cache.scans, cache.updates), (3, 0));
        Ok(())
    }

    #[tokio::test]
    async fn replacements_and_case_only_renames_keep_one_current_path() -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::write(root.join("file.rs"), "")?;
        let mut cache = FilePickerCache::new(&app.editor);
        let config = config();
        cache.initial(&root, &config, || false, |_| true);
        changed(&mut app.editor, &root.join("file.rs"), EventType::Delete);
        changed(&mut app.editor, &root.join("file.rs"), EventType::Create);
        assert_eq!(
            cache.refresh(&root, &config, None, || false),
            [root.join("file.rs")]
        );
        assert_eq!(cache.scans, 2);
        std::fs::rename(root.join("file.rs"), root.join("FILE.rs"))?;
        changed(&mut app.editor, &root.join("file.rs"), EventType::Delete);
        changed(&mut app.editor, &root.join("FILE.rs"), EventType::Create);
        assert_eq!(
            cache.refresh(&root, &config, None, || false),
            [root.join("FILE.rs")]
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_after_draining_does_not_lose_events() -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let mut cache = FilePickerCache::new(&app.editor);
        let config = config();
        cache.initial(&root, &config, || false, |_| true);
        std::fs::write(root.join("new.rs"), "")?;
        changed(&mut app.editor, &root.join("new.rs"), EventType::Create);
        let checks = Cell::new(0);
        cache.refresh(&root, &config, None, || {
            checks.set(checks.get() + 1);
            checks.get() >= 2
        });
        assert!(cache.index.is_none());
        assert_eq!(
            cache.refresh(&root, &config, None, || false),
            [root.join("new.rs")]
        );
        assert_eq!(cache.scans, 2);
        Ok(())
    }
    #[tokio::test]
    async fn cancellation_during_result_cloning_discards_the_candidate() -> anyhow::Result<()> {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        for name in ["a", "b", "c"] {
            std::fs::write(root.join(name), "")?;
        }
        let mut cache = FilePickerCache::new(&app.editor);
        let config = config();
        cache.initial(&root, &config, || false, |_| true);
        changed(&mut app.editor, &root.join("a"), EventType::Create);
        let checks = Cell::new(0);
        let rows = cache.refresh(&root, &config, None, || {
            checks.set(checks.get() + 1);
            checks.get() >= 4
        });
        assert!(rows.is_empty());
        assert!(cache.index.is_none());
        assert_eq!(cache.refresh(&root, &config, None, || false).len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn events_arriving_during_initial_streaming_survive_for_follow_up() -> anyhow::Result<()>
    {
        let mut app = application(false)?;
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::create_dir(root.join("a"))?;
        std::fs::create_dir(root.join("z"))?;
        std::fs::write(root.join("a/old.rs"), "")?;
        std::fs::write(root.join("z/last.rs"), "")?;
        let mut cache = FilePickerCache::new(&app.editor);
        let config = config();
        cache.initial(
            &root,
            &config,
            || false,
            |path| {
                if path == root.join("z/last.rs") {
                    std::fs::write(root.join("a/new.rs"), "").unwrap();
                    changed(&mut app.editor, &root.join("a/new.rs"), EventType::Create);
                }
                true
            },
        );
        assert_eq!(cache.index.as_ref().unwrap().paths().count(), 2);
        assert_eq!(cache.refresh(&root, &config, None, || false).len(), 3);
        assert_eq!((cache.scans, cache.updates), (1, 1));
        Ok(())
    }
}
