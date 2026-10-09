//! Shared language-server operations.

use anyhow::{bail, Context as _};

use crate::{DocumentId, Editor};

impl Editor {
    /// Stop selected servers attached to a document across all workspaces.
    /// An empty selection stops all initialized servers attached to the document.
    pub fn stop_language_servers(
        &mut self,
        document: DocumentId,
        servers: &[&str],
    ) -> anyhow::Result<()> {
        let doc = self
            .documents
            .get(&document)
            .context("Document no longer exists")?;
        let language_servers: Vec<_> = doc
            .language_servers()
            .map(|ls| ls.name().to_owned())
            .collect();
        let language_servers = if servers.is_empty() {
            language_servers
        } else {
            let (valid, invalid): (Vec<_>, Vec<_>) = servers
                .iter()
                .map(|name| name.to_string())
                .partition(|name| language_servers.contains(name));
            if !invalid.is_empty() {
                let s = if invalid.len() == 1 { "" } else { "s" };
                bail!("Unknown language server{s}: {}", invalid.join(", "));
            }
            valid
        };

        let mut detached = false;
        for name in language_servers {
            let server_ids: Vec<_> = self
                .language_servers
                .iter_clients()
                .filter(|client| client.name() == name)
                .map(|client| client.id())
                .collect();
            for server_id in server_ids {
                detached = true;
                self.cleanup_language_server(server_id);
            }
            self.language_servers.stop(&name);
            for doc in self.documents_mut() {
                if doc.remove_language_server_by_name(&name).is_some() {
                    doc.reset_all_inlay_hints();
                    doc.inlay_hints_oudated = true;
                    doc.clear_document_symbols();
                }
            }
        }
        if detached {
            if let Some(tasks) = self.invocation_tasks() {
                tasks.detached();
            }
        }
        Ok(())
    }

    /// Restart selected servers for a document and refresh all affected documents.
    /// An empty selection restarts all configured servers and ignores missing executables.
    pub fn restart_language_servers(
        &mut self,
        document: DocumentId,
        servers: &[&str],
    ) -> anyhow::Result<()> {
        let editor_config = self.config.load();
        let doc = self
            .documents
            .get(&document)
            .context("Document no longer exists")?;
        let config = doc
            .language
            .clone()
            .context("LSP not defined for the current document")?;
        let path = doc.path().map(ToOwned::to_owned);

        let language_servers: Vec<_> = config
            .language_servers
            .iter()
            .map(|ls| ls.name.as_str())
            .collect();
        let language_servers = if servers.is_empty() {
            language_servers
        } else {
            let (valid, invalid): (Vec<_>, Vec<_>) = servers
                .iter()
                .copied()
                .partition(|name| language_servers.contains(name));
            if !invalid.is_empty() {
                let s = if invalid.len() == 1 { "" } else { "s" };
                bail!("Unknown language server{s}: {}", invalid.join(", "));
            }
            valid
        };

        // Restart removes these clients from the registry before their exit notifications
        // arrive, so the exit handler can no longer clear their diagnostics.
        let old_server_ids: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| language_servers.contains(&client.name()))
            .map(|client| client.id())
            .collect();

        let mut detached = !old_server_ids.is_empty();
        for server_id in old_server_ids {
            self.cleanup_language_server(server_id);
        }

        let mut errors = Vec::new();
        for server in language_servers.iter() {
            match self
                .language_servers
                .restart_server(
                    server,
                    &config,
                    path.as_deref(),
                    &editor_config.workspace_lsp_roots,
                    editor_config.lsp.snippets,
                )
                .transpose()
            {
                // Ignore the executable-not-found error unless the server was explicitly requested
                // in the arguments.
                Err(lsp_client::Error::ExecutableNotFound(_)) if !servers.contains(server) => {}
                Err(err) => errors.push(err.to_string()),
                Ok(Some(_)) => detached = true,
                Ok(None) => (),
            }
        }

        // This collect is needed because refresh_language_server would need to re-borrow editor.
        let document_ids_to_refresh: Vec<DocumentId> = self
            .documents()
            .filter_map(|doc| match doc.language_config() {
                Some(config)
                    if config.language_servers.iter().any(|ls| {
                        language_servers
                            .iter()
                            .any(|restarted_ls| restarted_ls == &ls.name)
                    }) =>
                {
                    Some(doc.id())
                }
                _ => None,
            })
            .collect();

        for document_id in document_ids_to_refresh {
            self.refresh_language_servers(document_id);
        }

        if detached {
            if let Some(tasks) = self.invocation_tasks() {
                tasks.detached();
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Error restarting language servers: {}",
                errors.join(", ")
            ))
        }
    }
}
