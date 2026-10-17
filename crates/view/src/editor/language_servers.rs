//! Shared language-server operations.

use anyhow::{bail, Context as _};

use std::collections::HashMap;

use crate::{events::LanguageServerInitialized, DocumentId, Editor};
use lsp_client::LanguageServerId;
use workspace::trust::TrustQuery;

impl Editor {
    #[inline]
    pub fn language_server_by_id(
        &self,
        language_server_id: LanguageServerId,
    ) -> Option<&lsp_client::Client> {
        self.language_servers
            .get_by_id(language_server_id)
            .map(|client| &**client)
    }

    /// Refreshes the language server for a given document
    pub fn refresh_language_servers(&mut self, doc_id: DocumentId) {
        self.launch_language_servers(doc_id);
        crate::features::code_lenses::refresh_document(self, doc_id);
    }

    pub fn refresh_doc_language(&mut self, doc_id: DocumentId) {
        let loader = self.syn_loader.load();
        let doc = doc_mut!(self, &doc_id);
        doc.detect_language(&loader);
        doc.detect_editor_config();
        doc.detect_indent_and_line_ending();
        self.refresh_language_servers(doc_id);
        let doc = doc_mut!(self, &doc_id);
        let diagnostics = Editor::doc_diagnostics(&self.language_servers, &self.diagnostics, doc);
        doc.replace_language_server_diagnostics(diagnostics);
        doc.reset_all_inlay_hints();
        doc.clear_document_symbols();
        self.refresh_spelling(doc_id);
    }

    /// Launch a language server for a given document
    pub fn launch_language_servers(&mut self, doc_id: DocumentId) {
        if !self.config().lsp.enable {
            return;
        }
        // if doc doesn't have a URL it's a scratch buffer, ignore it
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        let Some(_) = doc.url() else {
            return;
        };
        let (lang, path) = (doc.language.clone(), doc.path());
        let config = doc.config.load();
        let root_dirs = &config.lsp.workspace_roots;

        let workspace = doc.workspace_root();
        let trust = self.workspace_trust.query(workspace, TrustQuery::Lsp);
        if !trust.is_trusted() {
            return;
        }

        // store only successfully started language servers
        let status_sender = self.handlers.status.clone();
        let language_servers = lang.as_ref().map_or_else(HashMap::default, |language| {
            self.language_servers
                .get(language, path, root_dirs, config.lsp.completion.snippets)
                .filter_map(|(lang, client)| match client {
                    Ok(client) => Some((lang, client)),
                    Err(err) => {
                        if let lsp_client::Error::ExecutableNotFound(err) = err {
                            // Silence by default since some language servers might just not be installed
                            log::debug!(
                                "Language server not found for `{}` {} {}", language.scope, lang, err,
                            );
                        } else {
                            status_sender.report_blocking(event::status::StatusMessage {
                                severity: event::status::Severity::Warning,
                                message: format!("Could not start language server {lang}: {err}").into(),
                            });
                            log::error!(
                                "Failed to initialize the language servers for `{}` - `{}` {{ {} }}",
                                language.scope,
                                lang,
                                err
                            );
                        }
                        None
                    }
                })
                .collect::<HashMap<_, _>>()
        });

        if language_servers.is_empty() && doc.language_servers.is_empty() {
            return;
        }

        doc.attach_language_servers(language_servers, &self.initialized_language_servers);
    }

    /// Publish editor readiness only after configuration is queued. Pending documents
    /// open with their latest snapshot before feature observers can request work.
    pub fn handle_language_server_initialized(&mut self, server_id: LanguageServerId) {
        let Some(server) = self.language_servers.get_by_id(server_id).cloned() else {
            log::debug!("ignoring initialization from removed server {server_id}");
            return;
        };
        if !server.is_initialized() || !self.initialized_language_servers.insert(server_id) {
            return;
        }
        if let Some(config) = server.config() {
            server.did_change_configuration(config.clone());
        }
        for doc in self.documents.values_mut() {
            doc.open_language_server_document(server_id);
        }
        event::dispatch(LanguageServerInitialized {
            editor: self,
            server_id,
        })
        .report_to(self.status_sender());
    }

    pub(crate) fn forget_language_server(&mut self, server_id: LanguageServerId) {
        self.initialized_language_servers.remove(&server_id);
    }

    pub(crate) fn language_server_is_ready(&self, server_id: LanguageServerId) -> bool {
        self.initialized_language_servers.contains(&server_id)
    }

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
