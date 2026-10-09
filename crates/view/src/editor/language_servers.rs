//! Shared language-server operations.

use anyhow::{bail, Context as _};

use crate::{DocumentId, Editor};

impl Editor {
    /// Clear document and workspace diagnostics, including unopened files, for one server.
    pub fn clear_language_server_diagnostics(&mut self, server_id: lsp_client::LanguageServerId) {
        self.diagnostics.retain(|_, diagnostics| {
            diagnostics.retain(|(_, provider)| provider.language_server_id() != Some(server_id));
            !diagnostics.is_empty()
        });
        for doc in self.documents_mut() {
            doc.clear_diagnostics_for_language_server(server_id);
            doc.pull_diagnostics.clear_for_server(server_id);
        }
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
            .language_config()
            .context("LSP not defined for the current document")?;

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

        let mut errors = Vec::new();
        for server in language_servers.iter() {
            match self
                .language_servers
                .restart_server(
                    server,
                    config,
                    doc.path(),
                    &editor_config.workspace_lsp_roots,
                    editor_config.lsp.snippets,
                )
                .transpose()
            {
                // Ignore the executable-not-found error unless the server was explicitly requested
                // in the arguments.
                Err(lsp_client::Error::ExecutableNotFound(_)) if !servers.contains(server) => {}
                Err(err) => errors.push(err.to_string()),
                _ => (),
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

        for server_id in old_server_ids {
            self.clear_language_server_diagnostics(server_id);
        }

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
