//! Workspace folders, configuration replies, and change notifications.

use super::Client;
use crate::lsp::{
    self, notification::DidChangeWorkspaceFolders, DidChangeWorkspaceFoldersParams, OneOf,
    WorkspaceFolder, WorkspaceFoldersChangeEvent,
};
use serde_json::Value;
use std::{path::Path, sync::Arc};

pub(super) fn workspace_for_uri(uri: lsp::Url) -> WorkspaceFolder {
    lsp::WorkspaceFolder {
        name: uri
            .path()
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .map(|basename| basename.to_string())
            .unwrap_or_default(),
        uri,
    }
}

impl Client {
    /// Determines whether this client can serve a resolved workspace and, when
    /// supported, adds its URI to the server's workspace folders.
    ///
    /// Before initialization completes, the capability decision is deferred to
    /// a background task. A `true` return means the caller may associate the
    /// document with this client immediately.
    pub fn try_add_workspace(
        self: &Arc<Self>,
        root_path: &Path,
        root_uri: Option<lsp::Url>,
        may_support_workspace: bool,
    ) -> bool {
        if self.root_path == root_path
            || root_uri.as_ref().is_some_and(|root_uri| {
                self.workspace_folders
                    .lock()
                    .iter()
                    .any(|workspace| &workspace.uri == root_uri)
            })
        {
            // workspace URI is already registered so we can use this client
            return true;
        }

        // this server definitely doesn't support multiple workspace, no need to check capabilities
        if !may_support_workspace {
            return false;
        }

        let Some(capabilities) = self.capabilities.get() else {
            let client = Arc::clone(self);
            // initialization hasn't finished yet, deal with this new root later
            // TODO: In the edgecase that a **new root** is added
            // for an LSP that **doesn't support workspace_folders** before initaliation is finished
            // the new roots are ignored.
            // That particular edgecase would require retroactively spawning new LSP
            // clients and therefore also require us to retroactively update the corresponding
            // documents LSP client handle. It's doable but a pretty weird edgecase so let's
            // wait and see if anyone ever runs into it.
            tokio::spawn(async move {
                client.initialize_notify.notified().await;
                if let Some(workspace_folders_caps) = client
                    .capabilities()
                    .workspace
                    .as_ref()
                    .and_then(|cap| cap.workspace_folders.as_ref())
                    .filter(|cap| cap.supported.unwrap_or(false))
                {
                    client.add_workspace_folder(
                        root_uri,
                        workspace_folders_caps.change_notifications.as_ref(),
                    );
                }
            });
            return true;
        };

        if let Some(workspace_folders_caps) = capabilities
            .workspace
            .as_ref()
            .and_then(|cap| cap.workspace_folders.as_ref())
            .filter(|cap| cap.supported.unwrap_or(false))
        {
            self.add_workspace_folder(
                root_uri,
                workspace_folders_caps.change_notifications.as_ref(),
            );
            true
        } else {
            // the server doesn't support multi workspaces, we need a new client
            false
        }
    }

    fn add_workspace_folder(
        &self,
        root_uri: Option<lsp::Url>,
        change_notifications: Option<&OneOf<bool, String>>,
    ) {
        // root_uri is None just means that there isn't really any LSP workspace
        // associated with this file. For servers that support multiple workspaces
        // there is just one server so we can always just use that shared instance.
        // No need to add a new workspace root here as there is no logical root for this file
        // let the server deal with this
        let Some(root_uri) = root_uri else {
            return;
        };

        // server supports workspace folders, let's add the new root to the list
        self.workspace_folders
            .lock()
            .push(workspace_for_uri(root_uri.clone()));
        if Some(&OneOf::Left(false)) == change_notifications {
            // server specifically opted out of DidWorkspaceChange notifications
            // let's assume the server will request the workspace folders itself
            // and that we can therefore reuse the client (but are done now)
            return;
        }
        self.did_change_workspace(vec![workspace_for_uri(root_uri)], Vec::new())
    }

    /// Resolve workspace/configuration items in request order. Empty or absent
    /// sections select the whole configuration; missing values serialize as null.
    /// Configuration is currently server-wide, so scope URIs do not affect lookup.
    pub fn configuration(&self, params: &lsp::ConfigurationParams) -> Vec<Option<&Value>> {
        params
            .items
            .iter()
            .map(|item| {
                let mut config = self.config()?;
                if let Some(section) = item
                    .section
                    .as_deref()
                    .filter(|section| !section.is_empty())
                {
                    for part in section.split('.') {
                        config = config.get(part)?;
                    }
                }
                Some(config)
            })
            .collect()
    }

    pub async fn workspace_folders(
        &self,
    ) -> parking_lot::MutexGuard<'_, Vec<lsp::WorkspaceFolder>> {
        self.workspace_folders.lock()
    }

    pub fn did_change_configuration(&self, settings: Value) {
        self.notify::<lsp::notification::DidChangeConfiguration>(
            lsp::DidChangeConfigurationParams { settings },
        )
    }

    pub fn did_change_workspace(&self, added: Vec<WorkspaceFolder>, removed: Vec<WorkspaceFolder>) {
        self.notify::<DidChangeWorkspaceFolders>(DidChangeWorkspaceFoldersParams {
            event: WorkspaceFoldersChangeEvent { added, removed },
        })
    }

    pub fn did_change_watched_files(&self, changes: Vec<lsp::FileEvent>) {
        self.notify::<lsp::notification::DidChangeWatchedFiles>(lsp::DidChangeWatchedFilesParams {
            changes,
        })
    }
}
