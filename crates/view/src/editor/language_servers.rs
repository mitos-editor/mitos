//! Shared language-server operations.

use anyhow::{bail, Context as _};

use crate::{DocumentId, Editor};

impl Editor {
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
