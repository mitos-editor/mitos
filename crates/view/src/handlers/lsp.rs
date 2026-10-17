mod workspace;

use crate::events::LanguageServerExited;
use crate::Editor;
use lsp_client::{lsp, LanguageServerId};

impl Editor {
    /// Clear this server's diagnostics, notify features, then remove its registry entry.
    /// Exit hooks still need the registered server and its document attachments.
    pub fn handle_language_server_exit(&mut self, server_id: LanguageServerId) {
        let Some(client) = self.language_servers.get_by_id(server_id).cloned() else {
            log::debug!("ignoring exit from removed language server {server_id}");
            return;
        };
        let name = client.name().to_owned();
        // Include documents attached while initialize was pending, including
        // documents in other workspace folders accepted by this client.
        let documents: Vec<_> = self
            .documents()
            .filter(|doc| {
                doc.language_servers
                    .get(&name)
                    .is_some_and(|server| server.id() == server_id)
            })
            .map(|doc| {
                (
                    doc.id(),
                    doc.language.clone(),
                    doc.path().map(ToOwned::to_owned),
                )
            })
            .collect();
        let roots = documents
            .iter()
            .filter_map(|(_, config, path)| {
                Some(
                    crate::language_servers::language_server_root(
                        config.as_ref()?,
                        path.as_deref(),
                        &self.config().lsp.workspace_roots,
                    )
                    .0,
                )
            })
            .collect::<Vec<_>>();
        let startup_failed = self.language_servers.record_exit(server_id, roots);
        self.cleanup_language_server(server_id);
        self.language_servers.remove_by_id(server_id);
        for (id, _, _) in &documents {
            if let Some(doc) = self.documents.get_mut(id) {
                doc.remove_language_server_by_name(&name);
            }
        }
        if startup_failed {
            let mut replacements = std::collections::BTreeSet::new();
            for (id, config, _) in documents {
                let group = config.as_ref().and_then(|config| {
                    config.language_server_groups().find(|group| {
                        group[0].alternative_group.is_some()
                            && group.iter().any(|server| server.name == name)
                    })
                });
                if let Some(group) = group {
                    self.refresh_language_servers(id);
                    if let Some(doc) = self.documents.get(&id) {
                        for candidate in group {
                            if doc.language_servers.contains_key(&candidate.name) {
                                replacements.insert(candidate.name.clone());
                            }
                        }
                    }
                }
            }
            let message = if replacements.is_empty() {
                format!("Language server {name} failed during startup; see the log for details")
            } else {
                format!(
                    "Language server {name} failed during startup; trying {}",
                    replacements.into_iter().collect::<Vec<_>>().join(", ")
                )
            };
            self.set_status(message);
        }
    }

    /// Clear diagnostics and notify features while the server and attachments still exist.
    /// Registry removal belongs to the caller so manual stop keeps its tombstone.
    pub(crate) fn cleanup_language_server(&mut self, server_id: LanguageServerId) {
        self.handlers.lsp_progress.remove_server(server_id);
        self.forget_language_server(server_id);
        self.clear_language_server_diagnostics(server_id);
        event::dispatch(LanguageServerExited {
            editor: self,
            server_id,
        })
        .report_to(self.status_sender());
    }

    pub fn execute_lsp_command(&mut self, command: lsp::Command, server_id: LanguageServerId) {
        let context = crate::action::LspActionContext::new(crate::current_ref!(self).1);
        crate::features::interactive::execute(self, command, server_id, context);
    }
}

/// Synchronize text before document observers may schedule requests against the new version.
pub(crate) fn synchronize_document(
    doc: &crate::Document,
    old_text: &editor_core::Rope,
    changes: &editor_core::ChangeSet,
) {
    for server in doc.language_servers() {
        server.text_document_did_change(doc.versioned_identifier(), old_text, doc.text(), changes);
    }
}

/// Close protocol documents before notifying observers about their removal.
pub(crate) fn close_document(doc: &crate::Document) {
    for server in doc.language_servers() {
        server.text_document_did_close(doc.identifier());
    }
}
