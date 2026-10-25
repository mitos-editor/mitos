//! Editor delivery and configuration adaptation for native file watching.
use crate::{
    callbacks::{EditorCallback, EditorCallbackSender},
    Editor,
};
use editor_config::WatchConfig;
pub use file_watch::{CanonicalPathBuf, Event, EventType, Events};
use std::path::{Path, PathBuf};
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
        crate::features::change_review::handle_file_events(self, events);
        let _ = self.file_watcher.events.send(events.clone());
        self.redraw().request();
    }
}

pub struct Watcher {
    native: file_watch::Watcher,
    events: broadcast::Sender<Events>,
    receiver: Option<mpsc::UnboundedReceiver<file_watch::EventBatch>>,
    callbacks: EditorCallbackSender,
    delivery: event::TaskController,
}

fn options(config: &WatchConfig, root: &Path) -> file_watch::Options {
    let root = stdx::path::canonicalize_existing(root);
    file_watch::Options {
        enable: config.enable,
        watch_vcs: config.watch_vcs,
        require_workspace: config.require_workspace,
        hidden: config.hidden,
        ignore: config.ignore,
        git_ignore: config.git_ignore,
        git_global: config.git_global,
        max_depth: config.max_depth,
        ignore_files: vec![
            loader::config_dir().join("ignore"),
            root.join(".mitos/ignore"),
        ],
        watch_ignore_files: vec![
            loader::config_dir().join("filesentryignore"),
            root.join(".mitos/filesentryignore"),
        ],
    }
}

impl Watcher {
    pub fn new(config: &WatchConfig, callbacks: EditorCallbackSender) -> Self {
        let (root, no_workspace) = workspace::find_workspace();
        let (native, receiver) =
            file_watch::Watcher::new(&options(config, &root), &root, !no_workspace);
        let mut watcher = Self {
            native,
            events: broadcast::channel(64).0,
            receiver: Some(receiver),
            callbacks,
            delivery: event::TaskController::new(),
        };
        watcher.start_delivery();
        watcher
    }

    fn start_delivery(&mut self) {
        if !self.native.is_running() {
            return;
        }
        let Some(receiver) = self.receiver.take() else {
            return;
        };
        tokio::spawn(event::cancelable_future(
            forward_events(
                receiver,
                self.callbacks.clone(),
                |batch: file_watch::EventBatch| {
                    if !batch.is_live() {
                        return None;
                    }
                    Some(Box::new(move |editor: &mut Editor| {
                        if batch.is_current(&editor.file_watcher.native) {
                            editor.handle_file_events(&batch.events);
                        }
                    }) as EditorCallback)
                },
            ),
            self.delivery.restart(),
        ));
    }

    /// Subscribe to batches after they have been delivered to the owning editor.
    pub fn subscribe(&self) -> broadcast::Receiver<Events> {
        self.events.subscribe()
    }

    pub fn reload(&mut self, config: &WatchConfig) {
        let (root, no_workspace) = workspace::find_workspace();
        self.native
            .reload(&options(config, &root), &root, !no_workspace);
        self.start_delivery();
    }

    pub fn is_watching(&self, path: &Path) -> bool {
        self.native.is_watching(path)
    }
    pub fn refresh_filter(&mut self) {
        self.native.refresh_filter();
    }
    pub fn add_root(&mut self, root: &Path) {
        self.native.add_root(root);
        self.start_delivery();
    }
    pub fn set_extra_watched_paths(&mut self, paths: Vec<PathBuf>) {
        self.native.set_extra_watched_paths(paths);
        self.start_delivery();
    }
    pub fn poll_extra_paths(&mut self) -> bool {
        self.native.poll_extra_paths()
    }
    pub fn is_vcs_path(&self, path: &Path) -> bool {
        self.native.is_vcs_path(path)
    }
}

async fn forward_events<T: Send + 'static>(
    mut events: mpsc::UnboundedReceiver<T>,
    callbacks: EditorCallbackSender,
    prepare: impl Fn(T) -> Option<EditorCallback> + Send + 'static,
) {
    while let Some(events) = events.recv().await {
        if let Some(callback) = prepare(events) {
            callbacks.send(callback).await;
        }
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
        let mut delivery = event::TaskController::new();
        let task = tokio::spawn(event::cancelable_future(
            forward_events(events, sender, |_| Some(Box::new(|_| {}))),
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
}
