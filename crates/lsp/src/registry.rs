//! Server selection, process registration, and explicit lifecycle transitions.

use crate::{
    file_event, lsp, Call, Client, Error, LanguageServerId, LanguageServerName, Result, Url,
};
use arc_swap::ArcSwap;
use editor_core::syntax::config::{
    LanguageConfiguration, LanguageServerConfiguration, RootMarkers,
};
use futures_util::stream::select_all::SelectAll;
use slotmap::SlotMap;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use stdx::path;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[derive(Debug)]
enum ServerInstances {
    Active(Vec<Arc<Client>>),
    Stopped,
}

impl ServerInstances {
    fn clients(&self) -> &[Arc<Client>] {
        match self {
            Self::Active(clients) => clients,
            Self::Stopped => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnavailableReason {
    StartupFailed,
    Exited,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct ServerWorkspace {
    server: LanguageServerName,
    root: PathBuf,
}

impl ServerWorkspace {
    fn new(server: &str, root: &Path) -> Self {
        Self {
            server: server.to_owned(),
            root: root.to_owned(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct AlternativeGroup {
    language: String,
    index: usize,
    root: PathBuf,
}

#[derive(Debug)]
pub struct Registry {
    inner: SlotMap<LanguageServerId, Arc<Client>>,
    instances: HashMap<LanguageServerName, ServerInstances>,
    syn_loader: Arc<ArcSwap<editor_core::syntax::Loader>>,
    unavailable: HashMap<ServerWorkspace, UnavailableReason>,
    alternative_selections: HashMap<AlternativeGroup, LanguageServerName>,
    pub incoming: SelectAll<UnboundedReceiverStream<(LanguageServerId, Call)>>,
    pub file_event_handler: file_event::Handler,
}

impl Registry {
    pub fn new(syn_loader: Arc<ArcSwap<editor_core::syntax::Loader>>) -> Self {
        Self {
            inner: SlotMap::with_key(),
            instances: HashMap::new(),
            syn_loader,
            unavailable: HashMap::new(),
            alternative_selections: HashMap::new(),
            incoming: SelectAll::new(),
            file_event_handler: file_event::Handler::new(),
        }
    }

    pub fn get_by_id(&self, id: LanguageServerId) -> Option<&Arc<Client>> {
        self.inner.get(id)
    }

    pub fn remove_by_id(&mut self, id: LanguageServerId) {
        let Some(client) = self.inner.remove(id) else {
            log::debug!("client was already removed");
            return;
        };
        self.file_event_handler.remove_client(id);
        let Some(ServerInstances::Active(instances)) = self.instances.get_mut(client.name()) else {
            unreachable!("registered clients must have an active server entry");
        };
        instances.retain(|ls| id != ls.id());
        if instances.is_empty() {
            self.instances.remove(client.name());
        }
    }

    fn start_client(
        &mut self,
        name: String,
        ls_config: &LanguageConfiguration,
        doc_path: Option<&std::path::Path>,
        root_dirs: &[PathBuf],
        enable_snippets: bool,
    ) -> Result<Arc<Client>, StartupError> {
        let syn_loader = self.syn_loader.load();
        let config = syn_loader
            .language_server_configs()
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("Language server '{name}' not defined"))?;
        let id = self.inner.try_insert_with_key(|id| {
            start_client(
                id,
                name,
                ls_config,
                config,
                doc_path,
                root_dirs,
                enable_snippets,
            )
            .map(|client| {
                self.incoming.push(UnboundedReceiverStream::new(client.1));
                client.0
            })
        })?;
        Ok(self.inner[id].clone())
    }

    /// If this method is called, all documents that have a reference to the language server have to refresh their language servers,
    /// See view::editor::Editor::refresh_language_servers
    pub fn restart_server(
        &mut self,
        name: &str,
        language_config: &LanguageConfiguration,
        doc_path: Option<&std::path::Path>,
        root_dirs: &[PathBuf],
        enable_snippets: bool,
    ) -> Option<Result<Arc<Client>>> {
        self.unavailable.retain(|key, _| key.server != name);
        if let Some(old_clients) = self.instances.remove(name) {
            if matches!(old_clients, ServerInstances::Stopped) {
                log::info!("restarting client for '{name}' which was manually stopped");
            } else {
                log::info!("stopping existing clients for '{name}'");
            }
            for old_client in old_clients.clients() {
                self.file_event_handler.remove_client(old_client.id());
                self.inner.remove(old_client.id());
                old_client.force_shutdown();
            }
        }
        let client = match self.start_client(
            name.to_string(),
            language_config,
            doc_path,
            root_dirs,
            enable_snippets,
        ) {
            Ok(client) => client,
            Err(StartupError::NoRequiredRootFound) => return None,
            Err(StartupError::Error(err)) => return Some(Err(err)),
        };
        self.instances.insert(
            name.to_owned(),
            ServerInstances::Active(vec![client.clone()]),
        );
        if let Some(server) = language_config
            .language_servers
            .iter()
            .find(|server| server.name == name)
            && let Some(group) = server.alternative_group
        {
            let (root, _) = language_server_root(language_config, doc_path, root_dirs);
            self.alternative_selections.insert(
                AlternativeGroup {
                    language: language_config.language_id.clone(),
                    index: group,
                    root,
                },
                name.to_owned(),
            );
        }

        Some(Ok(client))
    }

    pub fn stop(&mut self, name: &str) {
        if let Some(instances) = self.instances.get_mut(name) {
            let previous = std::mem::replace(instances, ServerInstances::Stopped);
            for client in previous.clients() {
                self.file_event_handler.remove_client(client.id());
                self.inner.remove(client.id());
                client.force_shutdown();
            }
        }
    }

    fn is_stopped(&self, name: &str) -> bool {
        matches!(self.instances.get(name), Some(ServerInstances::Stopped))
    }

    /// Remember failures per workspace. A running server's exit does not cause
    /// a switch to an alternative; explicit restart clears this state.
    pub fn record_exit(
        &mut self,
        id: LanguageServerId,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> bool {
        let Some(client) = self.inner.get(id) else {
            return false;
        };
        let initialized = client.is_initialized();
        let reason = if initialized {
            UnavailableReason::Exited
        } else {
            UnavailableReason::StartupFailed
        };
        for root in std::iter::once(client.root_path().to_owned()).chain(roots) {
            self.unavailable
                .insert(ServerWorkspace::new(client.name(), &root), reason);
        }
        !initialized
    }

    /// Stop and reset selected definitions without starting every alternative.
    pub fn reset(&mut self, names: &[&str]) {
        for name in names {
            self.stop(name);
            self.instances.remove(*name);
        }
        self.unavailable
            .retain(|key, _| !names.contains(&key.server.as_str()));
        self.alternative_selections
            .retain(|_, name| !names.contains(&name.as_str()));
    }

    fn existing_client(
        &self,
        name: &str,
        language: &LanguageConfiguration,
        doc_path: Option<&Path>,
        root_dirs: &[PathBuf],
    ) -> Option<Arc<Client>> {
        let (root, _) = language_server_root(language, doc_path, root_dirs);
        self.instances
            .get(name)?
            .clients()
            .iter()
            .enumerate()
            .find_map(|(index, client)| {
                // Pending clients have not negotiated workspace-folder support yet.
                // Do not attach a different root to an initialization that may fail.
                if !client.is_initialized() && client.root_path() != root {
                    return None;
                }
                client
                    .try_add_doc(
                        &language.roots,
                        language.workspace_lsp_roots.as_deref().unwrap_or(root_dirs),
                        doc_path,
                        index == 0,
                    )
                    .then(|| client.clone())
            })
    }

    pub fn get(
        &mut self,
        language_config: &LanguageConfiguration,
        doc_path: Option<&Path>,
        root_dirs: &[PathBuf],
        enable_snippets: bool,
    ) -> impl Iterator<Item = (LanguageServerName, Result<Arc<Client>>)> {
        let (root_path, _) = language_server_root(language_config, doc_path, root_dirs);
        let mut result = Vec::new();
        for group in language_config.language_server_groups() {
            let selection_key = group[0].alternative_group.map(|index| AlternativeGroup {
                language: language_config.language_id.clone(),
                index,
                root: root_path.clone(),
            });
            // Keep this language's selection stable without letting servers
            // chosen for another language override its preference order.
            if let Some(key) = &selection_key
                && let Some(name) = self.alternative_selections.get(key).cloned()
            {
                let server_key = ServerWorkspace::new(name.as_str(), &root_path);
                if group.iter().any(|server| server.name == name) {
                    if self.is_stopped(&name)
                        || self.unavailable.get(&server_key) == Some(&UnavailableReason::Exited)
                    {
                        continue;
                    }
                    if self.unavailable.get(&server_key) != Some(&UnavailableReason::StartupFailed)
                        && let Some(client) =
                            self.existing_client(&name, language_config, doc_path, root_dirs)
                    {
                        result.push((name, Ok(client)));
                        continue;
                    }
                }
                self.alternative_selections.remove(key);
            }
            for server in group {
                let name = &server.name;
                let server_key = ServerWorkspace::new(name.as_str(), &root_path);
                if self.is_stopped(name)
                    || (selection_key.is_some() && self.unavailable.contains_key(&server_key))
                {
                    continue;
                }
                let loader = self.syn_loader.load();
                if let Some(config) = loader.language_server_configs().get(name) {
                    match has_required_root(config, &root_path) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(error) => {
                            result.push((name.clone(), Err(error.into())));
                            continue;
                        }
                    }
                }
                drop(loader);
                if let Some(client) =
                    self.existing_client(name, language_config, doc_path, root_dirs)
                {
                    if let Some(key) = selection_key {
                        self.alternative_selections.insert(key, name.clone());
                    }
                    result.push((name.clone(), Ok(client)));
                    break;
                }
                match self.start_client(
                    name.clone(),
                    language_config,
                    doc_path,
                    root_dirs,
                    enable_snippets,
                ) {
                    Ok(client) => {
                        match self
                            .instances
                            .entry(name.clone())
                            .or_insert_with(|| ServerInstances::Active(Vec::new()))
                        {
                            ServerInstances::Active(clients) => clients.push(client.clone()),
                            ServerInstances::Stopped => {
                                unreachable!("stopped servers are not started automatically")
                            }
                        }
                        if let Some(key) = selection_key {
                            self.alternative_selections.insert(key, name.clone());
                        }
                        result.push((name.clone(), Ok(client)));
                        break;
                    }
                    Err(StartupError::NoRequiredRootFound) => {}
                    Err(StartupError::Error(err)) => {
                        if selection_key.is_some() && !matches!(err, Error::ExecutableNotFound(_)) {
                            self.unavailable
                                .insert(server_key, UnavailableReason::StartupFailed);
                        }
                        result.push((name.clone(), Err(err)));
                    }
                }
            }
        }
        result.into_iter()
    }

    pub fn iter_clients(&self) -> impl Iterator<Item = &Arc<Client>> {
        self.inner.values()
    }
}

struct NewClient(Arc<Client>, UnboundedReceiver<(LanguageServerId, Call)>);

enum StartupError {
    NoRequiredRootFound,
    Error(Error),
}

impl<T: Into<Error>> From<T> for StartupError {
    fn from(value: T) -> Self {
        StartupError::Error(value.into())
    }
}

/// start_client takes both a LanguageConfiguration and a LanguageServerConfiguration to ensure that
/// it is only called when it makes sense.
fn start_client(
    id: LanguageServerId,
    name: String,
    config: &LanguageConfiguration,
    ls_config: &LanguageServerConfiguration,
    doc_path: Option<&std::path::Path>,
    root_dirs: &[PathBuf],
    enable_snippets: bool,
) -> Result<NewClient, StartupError> {
    let (root_path, root_uri) = language_server_root(config, doc_path, root_dirs);
    if !has_required_root(ls_config, &root_path)? {
        log::warn!("The lsp {name:?} did not match its required-root-patterns at {root_path:?}");
        return Err(StartupError::NoRequiredRootFound);
    }

    let (client, incoming, initialize_notify) = Client::start(
        &ls_config.command,
        &ls_config.args,
        ls_config.config.clone(),
        &ls_config.environment,
        root_path,
        root_uri,
        id,
        name,
        ls_config.timeout,
    )?;

    let client = Arc::new(client);

    // Initialize the client asynchronously
    let _client = client.clone();
    tokio::spawn(async move {
        use futures_util::TryFutureExt;
        let value = _client
            .capabilities
            .get_or_try_init(|| {
                _client
                    .initialize(enable_snippets)
                    .map_ok(|response| response.capabilities)
            })
            .await;

        if let Err(e) = value {
            log::error!(
                "failed to initialize language server {}: {}",
                _client.name(),
                e
            );
            // Close the transport even when a server rejects initialize but stays
            // alive. Its exit drives startup fallback through the editor handler.
            _client.force_shutdown();
            return;
        }

        // next up, notify<initialized>
        _client.notify::<lsp::notification::Initialized>(lsp::InitializedParams {});

        initialize_notify.notify_one();
    });

    Ok(NewClient(client, incoming))
}

/// Resolve the root used for server startup and applicability checks.
pub fn language_server_root(
    config: &LanguageConfiguration,
    doc_path: Option<&Path>,
    root_dirs: &[PathBuf],
) -> (PathBuf, Option<Url>) {
    let (workspace, workspace_is_cwd) = workspace::find_workspace();
    let workspace = path::normalize(workspace);
    let root = find_lsp_workspace(
        doc_path
            .and_then(|path| path.parent().and_then(Path::to_str))
            .unwrap_or("."),
        &config.roots,
        config.workspace_lsp_roots.as_deref().unwrap_or(root_dirs),
        &workspace,
        workspace_is_cwd,
    );
    let root_path = root.clone().unwrap_or(workspace);
    let root_uri = root.and_then(|root| Url::from_file_path(root).ok());
    (root_path, root_uri)
}

pub fn has_required_root(
    config: &LanguageServerConfiguration,
    root: &Path,
) -> std::io::Result<bool> {
    let Some(patterns) = &config.required_root_patterns else {
        return Ok(true);
    };
    Ok(root
        .read_dir()?
        .flatten()
        .any(|entry| patterns.is_match(entry.file_name())))
}

/// Find an LSP workspace of a file using the following mechanism:
/// * if the file is outside `workspace` return `None`
/// * start at `file` and search the file tree upward
/// * stop the search at the first `root_dirs` entry that contains `file`
/// * if no `root_dirs` matches `file` stop at workspace
/// * Returns the top most directory that contains a `root_marker`
/// * If no root marker and we stopped at a `root_dirs` entry, return the directory we stopped at
/// * If we stopped at `workspace` instead and `workspace_is_cwd == false` return `None`
/// * If we stopped at `workspace` instead and `workspace_is_cwd == true` return `workspace`
pub fn find_lsp_workspace(
    file: &str,
    root_markers: &RootMarkers,
    root_dirs: &[PathBuf],
    workspace: &Path,
    workspace_is_cwd: bool,
) -> Option<PathBuf> {
    let file = std::path::Path::new(file);
    let mut file = if file.is_absolute() {
        file.to_path_buf()
    } else {
        let current_dir = stdx::env::current_working_dir();
        current_dir.join(file)
    };
    file = path::normalize(&file);

    if !file.starts_with(workspace) {
        return None;
    }

    let mut top_marker = None;
    for ancestor in file.ancestors() {
        let Ok(mut dir) = fs::read_dir(ancestor) else {
            continue;
        };

        if dir.any(|entry| {
            if let Ok(entry) = entry {
                return root_markers.is_match(entry.file_name());
            }
            false
        }) {
            top_marker = Some(ancestor);
        }

        if root_dirs
            .iter()
            .any(|root_dir| path::normalize(workspace.join(root_dir)) == ancestor)
        {
            // if the worskapce is the cwd do not search any higher for workspaces
            // but specify
            return Some(top_marker.unwrap_or(workspace).to_owned());
        }
        if ancestor == workspace {
            // if the workspace is the CWD, let the LSP decide what the workspace
            // is
            return top_marker
                .or_else(|| (!workspace_is_cwd).then_some(workspace))
                .map(Path::to_owned);
        }
    }

    debug_assert!(false, "workspace must be an ancestor of <file>");
    None
}
