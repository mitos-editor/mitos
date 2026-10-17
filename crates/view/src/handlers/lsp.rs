mod workspace;

use std::collections::btree_map::Entry;

use crate::events::{DiagnosticsDidChange, LanguageServerExited, LanguageServerInitialized};
use crate::Editor;
use editor_core::diagnostic::DiagnosticProvider;
use editor_core::Uri;
use lsp_client::{lsp, LanguageServerId};

impl Editor {
    /// Synchronize configuration and open documents before feature observers run.
    pub fn handle_language_server_initialized(&mut self, server_id: LanguageServerId) {
        let Some(language_server) = self.language_server_by_id(server_id) else {
            log::warn!("can't find language server with id `{server_id}`");
            return;
        };

        // Some servers expect configuration after initialization even though the
        // protocol does not require it. Keep this ahead of the document hooks.
        if let Some(config) = language_server.config() {
            language_server.did_change_configuration(config.clone());
        }
        for doc in self
            .documents()
            .filter(|doc| doc.supports_language_server(server_id))
        {
            let Some(url) = doc.url() else {
                continue;
            };
            let language_id = doc.language_id().map(ToOwned::to_owned).unwrap_or_default();
            language_server.text_document_did_open(url, doc.version(), doc.text(), language_id);
        }
        event::dispatch(LanguageServerInitialized {
            editor: self,
            server_id,
        })
        .report_to(self.status_sender());
    }

    /// Validate a push notification before applying the shared diagnostic update policy.
    pub fn handle_publish_diagnostics(
        &mut self,
        server_id: LanguageServerId,
        params: lsp::PublishDiagnosticsParams,
    ) {
        let uri = match Uri::try_from(params.uri) {
            Ok(uri) => uri,
            Err(err) => {
                log::error!("{err}");
                return;
            }
        };
        let Some(language_server) = self.language_server_by_id(server_id) else {
            log::warn!("can't find language server with id `{server_id}`");
            return;
        };
        if !language_server.is_initialized() {
            log::error!(
                "Discarding publishDiagnostic notification sent by an uninitialized server: {}",
                language_server.name()
            );
            return;
        }
        let provider = DiagnosticProvider::Lsp {
            server_id,
            identifier: None,
        };
        self.handle_lsp_diagnostics(&provider, uri, params.version, params.diagnostics);
    }

    /// Clear this server's diagnostics, notify features, then remove its registry entry.
    /// Exit hooks still need the registered server and its document attachments.
    pub fn handle_language_server_exit(&mut self, server_id: LanguageServerId) {
        if self.language_server_by_id(server_id).is_none() {
            log::warn!("can't find language server with id `{server_id}`");
            return;
        }
        self.cleanup_language_server(server_id);
        self.language_servers.remove_by_id(server_id);
    }

    /// Clear diagnostics and notify features while the server and attachments still exist.
    /// Registry removal belongs to the caller so manual stop keeps its tombstone.
    pub(crate) fn cleanup_language_server(&mut self, server_id: LanguageServerId) {
        self.diagnostics.retain(|_, diagnostics| {
            diagnostics.retain(|(_, provider)| provider.language_server_id() != Some(server_id));
            !diagnostics.is_empty()
        });
        for doc in self.documents_mut() {
            doc.clear_diagnostics_for_language_server(server_id);
        }
        event::dispatch(LanguageServerExited {
            editor: self,
            server_id,
        })
        .report_to(self.status_sender());
    }

    pub fn handle_lsp_diagnostics(
        &mut self,
        provider: &DiagnosticProvider,
        uri: Uri,
        version: Option<i32>,
        mut diagnostics: Vec<lsp::Diagnostic>,
    ) {
        let doc = self
            .documents
            .values_mut()
            .find(|doc| doc.uri().is_some_and(|u| u == uri));

        if let Some((version, doc)) = version.zip(doc.as_ref())
            && version != doc.version()
        {
            log::info!("Version ({version}) is out of date for {uri:?} (expected ({})), dropping PublishDiagnostic notification", doc.version());
            return;
        }

        let mut unchanged_diag_sources = Vec::new();
        if let Some((lang_conf, old_diagnostics)) = doc
            .as_ref()
            .and_then(|doc| Some((doc.language_config()?, self.diagnostics.get(&uri)?)))
        {
            if !lang_conf.persistent_diagnostic_sources.is_empty() {
                // Sort diagnostics first by severity and then by line numbers.
                // Note: The `lsp::DiagnosticSeverity` enum is already defined in decreasing order
                diagnostics.sort_by_key(|d| (d.severity, d.range.start));
            }
            for source in &lang_conf.persistent_diagnostic_sources {
                let new_diagnostics = diagnostics
                    .iter()
                    .filter(|d| d.source.as_ref() == Some(source));
                let old_diagnostics = old_diagnostics
                    .iter()
                    .filter(|(d, d_provider)| {
                        d_provider == provider && d.source.as_ref() == Some(source)
                    })
                    .map(|(d, _)| d);
                if new_diagnostics.eq(old_diagnostics) {
                    unchanged_diag_sources.push(source.clone())
                }
            }
        }

        let diagnostics = diagnostics.into_iter().map(|d| (d, provider.clone()));

        // Insert the original lsp::Diagnostics here because we may have no open document
        // for diagnostic message and so we can't calculate the exact position.
        // When using them later in the diagnostics picker, we calculate them on-demand.
        let diagnostics = match self.diagnostics.entry(uri) {
            Entry::Occupied(o) => {
                let current_diagnostics = o.into_mut();
                // there may entries of other language servers, which is why we can't overwrite the whole entry
                current_diagnostics.retain(|(_, d_provider)| d_provider != provider);
                current_diagnostics.extend(diagnostics);
                current_diagnostics
                // Sort diagnostics first by severity and then by line numbers.
            }
            Entry::Vacant(v) => v.insert(diagnostics.collect()),
        };

        // Sort diagnostics first by severity and then by line numbers.
        // Note: The `lsp::DiagnosticSeverity` enum is already defined in decreasing order
        diagnostics.sort_by_key(|(d, provider)| (d.severity, d.range.start, provider.clone()));

        if let Some(doc) = doc {
            let diagnostic_of_language_server_and_not_in_unchanged_sources =
                |diagnostic: &lsp::Diagnostic, d_provider: &DiagnosticProvider| {
                    d_provider == provider
                        && diagnostic
                            .source
                            .as_ref()
                            .is_none_or(|source| !unchanged_diag_sources.contains(source))
                };
            let diagnostics = Self::doc_diagnostics_with_filter(
                &self.language_servers,
                &self.diagnostics,
                doc,
                diagnostic_of_language_server_and_not_in_unchanged_sources,
            );
            doc.replace_diagnostics(diagnostics, &unchanged_diag_sources, Some(provider));

            let doc = doc.id();
            event::dispatch(DiagnosticsDidChange { editor: self, doc })
                .report_to(self.status_sender());
        }
    }

    pub fn execute_lsp_command(&mut self, command: lsp::Command, server_id: LanguageServerId) {
        // the command is executed on the server and communicated back
        // to the client asynchronously using workspace edits
        let Some(future) = self
            .language_server_by_id(server_id)
            .and_then(|server| server.command(command))
        else {
            self.set_error(|| "Language server does not support executing commands");
            return;
        };

        tokio::spawn(async move {
            if let Err(err) = future.await {
                log::log!(
                    lsp_client::error_log_level(&err),
                    "Error executing LSP command: {err}"
                );
            }
        });
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
