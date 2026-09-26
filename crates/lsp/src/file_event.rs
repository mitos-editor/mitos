//! Batched LSP file notifications.
//!
//! Adapted from work by Pascal Kuthe and Blaž Hrastnik in
//! [Helix PR #14544](https://github.com/helix-editor/helix/pull/14544).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Weak,
};

use globset::{Glob, GlobBuilder, GlobSet};
use tokio::sync::mpsc;

use crate::{lsp, Client, LanguageServerId};

enum Event {
    FilesChanged(Vec<(PathBuf, lsp::FileChangeType)>),
    Register {
        client: Weak<Client>,
        registration_id: String,
        options: lsp::DidChangeWatchedFilesRegistrationOptions,
    },
    Unregister {
        client_id: LanguageServerId,
        registration_id: String,
    },
    RemoveClient {
        client_id: LanguageServerId,
    },
}

#[derive(Default)]
struct ClientState {
    client: Weak<Client>,
    registrations: HashMap<String, Vec<(Glob, lsp::WatchKind)>>,
    pending: Vec<lsp::FileEvent>,
}

#[derive(Default)]
struct State {
    clients: HashMap<LanguageServerId, ClientState>,
    matcher: GlobSet,
    interests: Vec<(LanguageServerId, lsp::WatchKind)>,
    candidates: Vec<usize>,
}

/// Relative bases are literal filesystem paths, even when they contain glob syntax.
fn watcher_glob(pattern: lsp::GlobPattern) -> anyhow::Result<Glob> {
    let pattern = match pattern {
        lsp::GlobPattern::String(pattern) => pattern,
        lsp::GlobPattern::Relative(pattern) => {
            let uri = match pattern.base_uri {
                lsp::OneOf::Left(folder) => folder.uri,
                lsp::OneOf::Right(uri) => uri,
            };
            let path = uri
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("invalid file watcher base URI: {uri}"))?;
            let path = stdx::path::canonicalize_existing(&path);
            let path = path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("file watcher base must be UTF-8"))?;
            #[cfg(windows)]
            let path = path.replace('\\', "/");
            format!(
                "{}/{}",
                globset::escape(path.trim_end_matches('/')),
                pattern.pattern
            )
        }
    };
    Ok(GlobBuilder::new(&pattern).literal_separator(true).build()?)
}

impl State {
    fn register(
        &mut self,
        id: LanguageServerId,
        client: Weak<Client>,
        registration: String,
        options: lsp::DidChangeWatchedFilesRegistrationOptions,
    ) {
        let state = self.clients.entry(id).or_default();
        if !state.client.ptr_eq(&client) {
            *state = ClientState::default();
        }
        state.client = client;
        let interests = options
            .watchers
            .into_iter()
            .filter_map(|watcher| {
                let flags = watcher.kind.unwrap_or(lsp::WatchKind::all());
                if flags.is_empty() {
                    return None;
                }
                match watcher_glob(watcher.glob_pattern) {
                    Ok(glob) => Some((glob, flags)),
                    Err(err) => {
                        log::warn!("invalid LSP file watcher: {err}");
                        None
                    }
                }
            })
            .collect();
        state.registrations.insert(registration, interests);
        self.rebuild();
    }

    fn unregister(&mut self, id: LanguageServerId, registration: &str) {
        if let Some(client) = self.clients.get_mut(&id) {
            client.registrations.remove(registration);
            if client.registrations.is_empty() {
                self.clients.remove(&id);
            }
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let mut builder = GlobSet::builder();
        let mut interests = Vec::new();
        for (&id, client) in &self.clients {
            for (glob, flags) in client.registrations.values().flatten() {
                builder.add(glob.clone());
                interests.push((id, *flags));
            }
        }
        match builder.build() {
            Ok(matcher) => {
                self.matcher = matcher;
                self.interests = interests;
            }
            Err(err) => {
                // Never retain an old matcher referring to removed clients.
                self.matcher = GlobSet::empty();
                self.interests.clear();
                log::error!("failed to build file watcher patterns: {err}");
            }
        }
    }

    fn queue<'a>(&mut self, events: impl IntoIterator<Item = (&'a Path, lsp::FileChangeType)>) {
        for (path, ty) in events {
            let flag = match ty {
                lsp::FileChangeType::CREATED => lsp::WatchKind::Create,
                lsp::FileChangeType::DELETED => lsp::WatchKind::Delete,
                lsp::FileChangeType::CHANGED => lsp::WatchKind::Change,
                _ => continue,
            };
            let Ok(uri) = lsp::Url::from_file_path(path) else {
                continue;
            };
            self.matcher.matches_into(path, &mut self.candidates);
            for &candidate in &self.candidates {
                let (id, flags) = self.interests[candidate];
                if !flags.contains(flag) {
                    continue;
                }
                let event = lsp::FileEvent {
                    uri: uri.clone(),
                    typ: ty,
                };
                let pending = &mut self.clients.get_mut(&id).unwrap().pending;
                // Overlapping registrations should deliver a change only once per server.
                if !pending.contains(&event) {
                    pending.push(event);
                }
            }
        }
    }

    fn notify<'a>(&mut self, events: impl IntoIterator<Item = (&'a Path, lsp::FileChangeType)>) {
        self.queue(events);
        let mut removed = false;
        self.clients.retain(|_, state| {
            let Some(client) = state.client.upgrade() else {
                removed = true;
                return false;
            };
            if !state.pending.is_empty() {
                client.did_change_watched_files(std::mem::take(&mut state.pending));
            }
            true
        });
        if removed {
            self.rebuild();
        }
    }
}

/// Routes native changes and editor writes to registered language servers.
#[derive(Clone, Debug)]
pub struct Handler {
    tx: mpsc::UnboundedSender<Event>,
}

impl Default for Handler {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(Self::run(rx));
        Self { tx }
    }

    pub fn register(
        &self,
        client: Weak<Client>,
        registration_id: String,
        options: lsp::DidChangeWatchedFilesRegistrationOptions,
    ) {
        let _ = self.tx.send(Event::Register {
            client,
            registration_id,
            options,
        });
    }

    pub fn unregister(&self, client_id: LanguageServerId, registration_id: String) {
        let _ = self.tx.send(Event::Unregister {
            client_id,
            registration_id,
        });
    }

    /// Submit one editor-observed change, including paths that no longer exist.
    pub fn file_changed(&self, path: PathBuf, ty: lsp::FileChangeType) {
        self.files_changed([(stdx::path::canonicalize_existing(&path), ty)]);
    }

    /// Submit an ordered batch of absolute paths with resolved existing ancestors.
    /// The caller supplies normalized paths, without exposing native watcher types.
    /// Overlapping registrations deliver each path/change pair only once per batch.
    pub fn files_changed(&self, changes: impl IntoIterator<Item = (PathBuf, lsp::FileChangeType)>) {
        let changes: Vec<_> = changes.into_iter().collect();
        if !changes.is_empty() {
            let _ = self.tx.send(Event::FilesChanged(changes));
        }
    }

    pub fn remove_client(&self, client_id: LanguageServerId) {
        let _ = self.tx.send(Event::RemoveClient { client_id });
    }

    async fn run(mut rx: mpsc::UnboundedReceiver<Event>) {
        let mut state = State::default();
        while let Some(event) = rx.recv().await {
            match event {
                Event::FilesChanged(changes) => {
                    state.notify(changes.iter().map(|(path, ty)| (path.as_path(), *ty)))
                }
                Event::Register {
                    client,
                    registration_id,
                    options,
                } => {
                    if let Some(strong) = client.upgrade() {
                        state.register(strong.id(), client, registration_id, options);
                    }
                }
                Event::Unregister {
                    client_id,
                    registration_id,
                } => state.unregister(client_id, &registration_id),
                Event::RemoveClient { client_id } => {
                    state.clients.remove(&client_id);
                    state.rebuild();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(
        pattern: &str,
        kind: Option<lsp::WatchKind>,
    ) -> lsp::DidChangeWatchedFilesRegistrationOptions {
        lsp::DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![lsp::FileSystemWatcher {
                glob_pattern: lsp::GlobPattern::String(pattern.into()),
                kind,
            }],
        }
    }

    #[test]
    fn overlapping_registrations_respect_kinds_and_unregister_independently() {
        let mut state = State::default();
        let id = LanguageServerId::default();
        state.register(
            id,
            Weak::new(),
            "changes".into(),
            registration("**/*.rs", Some(lsp::WatchKind::Change)),
        );
        state.register(id, Weak::new(), "all".into(), registration("**/*.rs", None));
        let path = std::env::temp_dir().join("watched.rs");
        state.queue([
            (path.as_path(), lsp::FileChangeType::CREATED),
            (path.as_path(), lsp::FileChangeType::CHANGED),
            (path.as_path(), lsp::FileChangeType::DELETED),
        ]);
        let pending = &mut state.clients.get_mut(&id).unwrap().pending;
        assert_eq!(
            pending.iter().map(|event| event.typ).collect::<Vec<_>>(),
            [
                lsp::FileChangeType::CREATED,
                lsp::FileChangeType::CHANGED,
                lsp::FileChangeType::DELETED
            ]
        );
        pending.clear();
        state.unregister(id, "all");
        state.register(
            id,
            Weak::new(),
            "new".into(),
            registration("**/*.toml", None),
        );
        state.unregister(id, "changes");
        let config = std::env::temp_dir().join("Cargo.toml");
        state.queue([
            (path.as_path(), lsp::FileChangeType::CHANGED),
            (config.as_path(), lsp::FileChangeType::CREATED),
        ]);
        assert_eq!(state.clients[&id].pending.len(), 1);
        assert_eq!(
            state.clients[&id].pending[0].uri,
            lsp::Url::from_file_path(config).unwrap()
        );
    }

    #[test]
    fn reregister_replaces_only_the_named_registration() {
        let mut state = State::default();
        let id = LanguageServerId::default();
        state.register(
            id,
            Weak::new(),
            "first".into(),
            registration("**/*.rs", None),
        );
        state.register(
            id,
            Weak::new(),
            "second".into(),
            registration("**/*.toml", None),
        );
        state.register(
            id,
            Weak::new(),
            "first".into(),
            registration("**/*.md", None),
        );
        let paths: Vec<_> = ["a.rs", "a.toml", "a.md"]
            .map(|name| std::env::temp_dir().join(name))
            .into();
        state.queue(
            paths
                .iter()
                .map(|path| (path.as_path(), lsp::FileChangeType::CHANGED)),
        );
        assert_eq!(state.clients[&id].pending.len(), 2);
    }

    #[test]
    fn relative_patterns_escape_the_base_and_obey_directory_boundaries() {
        let base = stdx::path::canonicalize_existing(&std::env::temp_dir()).join("project[1]");
        let glob = watcher_glob(lsp::GlobPattern::Relative(lsp::RelativePattern {
            base_uri: lsp::OneOf::Right(lsp::Url::from_directory_path(&base).unwrap()),
            pattern: "*.rs".into(),
        }))
        .unwrap()
        .compile_matcher();
        assert!(glob.is_match(base.join("a.rs")));
        assert!(!glob.is_match(base.join("nested/a.rs")));
        assert!(!glob.is_match(std::env::temp_dir().join("project1/a.rs")));
    }
}
