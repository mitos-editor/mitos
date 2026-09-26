//! Editor operations requested by language servers. Transport dispatch and replies
//! remain with the frontend; configuration lookup lives on the LSP client.

use std::sync::Arc;

use lsp_client::{
    jsonrpc,
    lsp::{self, notification::Notification},
    LanguageServerId,
};

use crate::Editor;

impl Editor {
    /// Validate the requesting server and apply edits using its negotiated encoding.
    pub fn handle_workspace_edit(
        &mut self,
        server_id: LanguageServerId,
        params: lsp::ApplyWorkspaceEditParams,
    ) -> Result<lsp::ApplyWorkspaceEditResponse, jsonrpc::Error> {
        let server = self
            .language_server_by_id(server_id)
            .ok_or_else(|| jsonrpc::Error {
                code: jsonrpc::ErrorCode::InvalidRequest,
                message: format!("Unknown language server: {server_id}"),
                data: None,
            })?;
        if !server.is_initialized() {
            return Err(jsonrpc::Error {
                code: jsonrpc::ErrorCode::InvalidRequest,
                message: "Server must be initialized to request workspace edits".to_string(),
                data: None,
            });
        }
        let result = self.apply_workspace_edit(server.offset_encoding(), &params.edit);
        Ok(lsp::ApplyWorkspaceEditResponse {
            applied: result.is_ok(),
            failure_reason: result.as_ref().err().map(|err| err.kind.to_string()),
            failed_change: result
                .as_ref()
                .err()
                .map(|err| err.failed_change_idx as u32),
        })
    }

    /// Register supported dynamic capabilities and their required filesystem roots.
    /// Unsupported registrations are acknowledged for compatibility with servers
    /// that send them despite the advertised client capabilities.
    pub fn register_language_server_capabilities(
        &mut self,
        server_id: LanguageServerId,
        params: lsp::RegistrationParams,
    ) {
        if let Some(client) = self.language_servers.get_by_id(server_id) {
            for reg in params.registrations {
                match reg.method.as_str() {
                    lsp::notification::DidChangeWatchedFiles::METHOD => {
                        let Some(options) = reg.register_options else {
                            continue;
                        };
                        let ops: lsp::DidChangeWatchedFilesRegistrationOptions =
                            match serde_json::from_value(options) {
                                Ok(ops) => ops,
                                Err(err) => {
                                    log::warn!("Failed to deserialize DidChangeWatchedFilesRegistrationOptions: {err}");
                                    continue;
                                }
                            };
                        for watch in &ops.watchers {
                            if let lsp::GlobPattern::Relative(pattern) = &watch.glob_pattern {
                                let base_url = match &pattern.base_uri {
                                    lsp::OneOf::Left(folder) => &folder.uri,
                                    lsp::OneOf::Right(url) => url,
                                };
                                let Ok(base_dir) = base_url.to_file_path() else {
                                    continue;
                                };
                                self.file_watcher.add_root(&base_dir);
                            }
                        }
                        self.language_servers.file_event_handler.register(
                            Arc::downgrade(client),
                            reg.id,
                            ops,
                        )
                    }
                    _ => {
                        // Language Servers based on the `vscode-languageserver-node` library often send
                        // client/registerCapability even though we do not enable dynamic registration
                        // for most capabilities. We should send a MethodNotFound JSONRPC error in this
                        // case but that rejects the registration promise in the server which causes an
                        // exit. So we work around this by ignoring the request and sending back an OK
                        // response.
                        log::warn!("Ignoring a client/registerCapability request because dynamic capability registration is not enabled. Please report this upstream to the language server");
                    }
                }
            }
        }
    }

    /// Remove the named registrations for this server, preserving other interests.
    pub fn unregister_language_server_capabilities(
        &mut self,
        server_id: LanguageServerId,
        params: lsp::UnregistrationParams,
    ) {
        for unreg in params.unregisterations {
            match unreg.method.as_str() {
                lsp::notification::DidChangeWatchedFiles::METHOD => {
                    self.language_servers
                        .file_event_handler
                        .unregister(server_id, unreg.id);
                }
                _ => {
                    log::warn!(
                        "Received unregistration request for unsupported method: {}",
                        unreg.method
                    );
                }
            }
        }
    }
}
