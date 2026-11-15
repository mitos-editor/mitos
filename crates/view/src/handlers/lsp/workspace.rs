//! Editor operations requested by language servers. Transport dispatch and replies
//! remain with the frontend; configuration lookup lives on the LSP client.

use lsp_client::UriExt as _;
use std::sync::Arc;

use lsp_client::{
    jsonrpc,
    lsp::{self, notification::Notification},
    LanguageServerId,
};

use crate::Editor;

impl Editor {
    /// Server requests wait for annotation consent before replying or applying any changes.
    pub fn request_workspace_edit_from_server(
        &mut self,
        server: LanguageServerId,
        params: lsp::ApplyWorkspaceEditParams,
        completed: impl FnOnce(&mut Editor, Result<lsp::ApplyWorkspaceEditResponse, jsonrpc::Error>)
            + Send
            + 'static,
    ) {
        if !self
            .language_server_by_id(server)
            .is_some_and(|server| server.is_initialized())
        {
            completed(
                self,
                Err(jsonrpc::Error {
                    code: jsonrpc::ErrorCode::InvalidRequest,
                    message: "Server must be active and initialized to request workspace edits"
                        .into(),
                    data: None,
                }),
            );
            return;
        }
        let context = crate::workspace_edit::confirmation::EditContext {
            label: params.label,
            is_refactoring: params
                .metadata
                .is_some_and(|metadata| metadata.is_refactoring == Some(true)),
        };
        self.request_workspace_edit_with_context(
            server,
            params.edit,
            context,
            move |editor, result| {
                let failed_change = result
                    .as_ref()
                    .err()
                    .and_then(|error| error.downcast_ref::<crate::workspace_edit::ApplyEditError>())
                    .map(|error| error.failed_change_idx as u32);
                let response = lsp::ApplyWorkspaceEditResponse {
                    applied: result.as_ref().is_ok_and(|applied| *applied),
                    failure_reason: match &result {
                        Ok(true) => None,
                        Ok(false) => Some("Workspace edit was canceled".into()),
                        Err(error) => Some(
                            error
                                .downcast_ref::<crate::workspace_edit::ApplyEditError>()
                                .map_or_else(|| error.to_string(), |error| error.kind.to_string()),
                        ),
                    },
                    failed_change,
                };
                completed(editor, Ok(response));
            },
        );
    }

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
                        let roots = ops
                            .watchers
                            .iter()
                            .filter_map(|watch| {
                                let lsp::GlobPattern::Relative(pattern) = &watch.glob_pattern
                                else {
                                    return None;
                                };
                                let base = match &pattern.base_uri {
                                    lsp::OneOf::Left(folder) => &folder.uri,
                                    lsp::OneOf::Right(url) => url,
                                };
                                base.to_file_path().ok()
                            })
                            .collect();
                        self.file_watcher.set_language_server_roots(
                            server_id,
                            reg.id.clone(),
                            roots,
                        );
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
                    self.file_watcher
                        .remove_language_server_registration(server_id, &unreg.id);
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
