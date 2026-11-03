//! Editor delivery and configuration adaptation for native file watching.
use crate::{
    callbacks::{EditorCallback, EditorCallbackSender},
    DocumentId, Editor,
};
use editor_config::WatchConfig;
pub use file_watch::{CanonicalPathBuf, Demand, Event, EventType, Events};
use lsp_client::LanguageServerId;
use std::collections::HashMap;
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

#[derive(Debug, PartialEq, Eq, Hash)]
enum Owner {
    Vcs,
    Document(DocumentId),
    LanguageServer(LanguageServerId, String),
    Explicit,
}

pub struct Watcher {
    native: file_watch::Watcher,
    demands: HashMap<Owner, Demand>,
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
    pub fn new(
        config: &WatchConfig,
        workspace: &workspace::ResolvedWorkspace,
        callbacks: EditorCallbackSender,
    ) -> Self {
        let root = workspace.root();
        let (native, receiver) =
            file_watch::Watcher::new(&options(config, root), root, workspace.is_workspace());
        let mut watcher = Self {
            native,
            demands: HashMap::new(),
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
                        crate::callbacks::CallbackOutcome::Redraw
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

    pub fn reload(&mut self, config: &WatchConfig, workspace: &workspace::ResolvedWorkspace) {
        let root = workspace.root();
        self.native
            .reload(&options(config, root), root, workspace.is_workspace());
        self.start_delivery();
    }

    pub fn is_watching(&self, path: &Path) -> bool {
        self.native.is_watching(path)
    }
    pub fn refresh_filter(&mut self) {
        self.native.refresh_filter();
    }
    fn project_demands(&mut self) {
        let mut union = Demand::default();
        for demand in self.demands.values() {
            union.roots.extend(demand.roots.iter().cloned());
            union.metadata.extend(demand.metadata.iter().cloned());
            union.files.extend(demand.files.iter().cloned());
        }
        self.native.set_paths(union);
        self.start_delivery();
    }

    /// Add a root owned by this watcher's explicit/manual demand.
    pub fn add_root(&mut self, root: &Path) {
        let Ok(root) = root.canonicalize() else {
            return;
        };
        if !root.is_dir() {
            return;
        }
        let roots = &mut self.demands.entry(Owner::Explicit).or_default().roots;
        if roots.contains(&root) {
            return;
        }
        roots.push(root);
        self.project_demands();
    }

    pub fn set_extra_watched_paths(&mut self, metadata: Vec<PathBuf>, files: Vec<PathBuf>) {
        let demand = self.demands.entry(Owner::Explicit).or_default();
        demand.metadata = metadata;
        demand.files = files;
        self.project_demands();
    }

    pub(crate) fn set_document_demands(
        &mut self,
        mut workspace: Demand,
        documents: Vec<(DocumentId, Demand)>,
    ) {
        self.demands
            .retain(|owner, _| !matches!(owner, Owner::Vcs | Owner::Document(_)));
        workspace.metadata.sort();
        workspace.metadata.dedup();
        workspace.files.sort();
        workspace.files.dedup();
        if !workspace.is_empty() {
            self.demands.insert(Owner::Vcs, workspace);
        }
        self.demands.extend(
            documents
                .into_iter()
                .map(|(id, demand)| (Owner::Document(id), demand)),
        );
        self.project_demands();
    }

    pub(crate) fn set_language_server_roots(
        &mut self,
        server: LanguageServerId,
        registration: String,
        roots: Vec<PathBuf>,
    ) {
        let roots: Vec<_> = roots
            .into_iter()
            .filter_map(|root| root.canonicalize().ok())
            .filter(|root| root.is_dir())
            .collect();
        let owner = Owner::LanguageServer(server, registration);
        if roots.is_empty() {
            self.demands.remove(&owner);
        } else {
            self.demands.insert(
                owner,
                Demand {
                    roots,
                    ..Default::default()
                },
            );
        }
        self.project_demands();
    }

    pub(crate) fn remove_language_server_registration(
        &mut self,
        server: LanguageServerId,
        registration: &str,
    ) {
        self.demands
            .remove(&Owner::LanguageServer(server, registration.to_owned()));
        self.project_demands();
    }

    pub(crate) fn remove_language_server(&mut self, server: LanguageServerId) {
        self.demands
            .retain(|owner, _| !matches!(owner, Owner::LanguageServer(id, _) if *id == server));
        self.project_demands();
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
            forward_events(events, sender, |_| Some(Box::new(|_| ().into()))),
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
