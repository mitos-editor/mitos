//! Shared language-server operations.

use anyhow::{bail, Context as _};

use crate::{DocumentId, Editor};

impl Editor {
    /// Stop selected servers attached to a document across all workspaces.
    /// An empty selection stops all servers attached to the document, including startup.
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
            .language_servers
            .values()
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

        for name in language_servers {
            let server_ids: Vec<_> = self
                .language_servers
                .iter_clients()
                .filter(|client| client.name() == name)
                .map(|client| client.id())
                .collect();
            for server_id in server_ids {
                self.cleanup_language_server(server_id);
            }
            self.language_servers.stop(&name);
            for doc in self.documents_mut() {
                if doc.remove_language_server_by_name(&name).is_some() {
                    doc.reset_all_inlay_hints();
                    doc.clear_document_symbols();
                }
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

        for group in config
            .language_server_groups()
            .filter(|group| group[0].alternative_group.is_some())
        {
            if servers
                .iter()
                .filter(|name| group.iter().any(|server| server.name == **name))
                .count()
                > 1
            {
                bail!(
                    "Choose one language-server alternative from: {}",
                    group
                        .iter()
                        .map(|server| server.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }

        // Selecting one member restarts its whole alternatives group so an old
        // selected member cannot keep running alongside the requested server.
        let language_servers: Vec<_> = config
            .language_server_groups()
            .filter(|group| {
                group
                    .iter()
                    .any(|server| language_servers.contains(&server.name.as_str()))
            })
            .flat_map(|group| group.iter().map(|server| server.name.as_str()))
            .collect();

        // Restart removes these clients from the registry before their exit notifications
        // arrive, so the exit handler can no longer clear their diagnostics.
        let old_server_ids: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| language_servers.contains(&client.name()))
            .map(|client| client.id())
            .collect();

        for server_id in old_server_ids {
            self.cleanup_language_server(server_id);
        }

        self.language_servers.reset(&language_servers);
        let mut errors = Vec::new();
        // With no explicit names, refresh below selects one candidate per group.
        // An explicit name is started first, overriding the group's preference.
        for server in servers {
            if let Err(err) = self
                .language_servers
                .restart_server(
                    server,
                    &config,
                    path.as_deref(),
                    &editor_config.lsp.workspace_roots,
                    editor_config.lsp.completion.snippets,
                )
                .transpose()
            {
                errors.push(err.to_string());
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
