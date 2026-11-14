//! Shared inbound LSP decoding, editor effects and request replies.
use crate::{features::lsp_progress::ProgressUpdate, Editor};
use log::{error, info, warn};
use lsp_client::{jsonrpc, lsp, LanguageServerId};
use serde_json::{json, Value};

/// Frontend choices left after the editor has applied protocol semantics.
pub enum HostAction {
    Progress(ProgressUpdate),
    ShowMessage(lsp::ShowMessageParams),
    ShowMessageRequest {
        message: String,
        actions: Vec<lsp::MessageActionItem>,
        reply: DeferredReply,
    },
    ShowDocument {
        params: lsp::ShowDocumentParams,
        offset_encoding: lsp_client::OffsetEncoding,
        reply: DeferredReply,
    },
}

/// A single reply addressed to the original server instance. Dropping it stays silent,
/// matching an abandoned or replaced prompt; it does not keep the client alive.
pub struct DeferredReply {
    server_id: LanguageServerId,
    client: std::sync::Weak<lsp_client::Client>,
    id: jsonrpc::Id,
}

impl DeferredReply {
    fn new(editor: &Editor, server_id: LanguageServerId, id: jsonrpc::Id) -> Self {
        let client = editor
            .language_servers
            .get_by_id(server_id)
            .map(std::sync::Arc::downgrade)
            .unwrap_or_default();
        Self {
            server_id,
            client,
            id,
        }
    }
    pub fn complete(self, editor: &Editor, result: Result<Value, jsonrpc::Error>) {
        if !editor.language_servers.is_active(self.server_id) {
            return;
        }
        let Some(client) = editor.language_server_by_id(self.server_id) else {
            warn!("can't find language server with id `{}`", self.server_id);
            return;
        };
        // Registry IDs are local to one editor; a different editor may reuse the same ID.
        if !std::ptr::eq(self.client.as_ptr(), client) {
            return;
        }
        if let Err(error) = client.reply(self.id.clone(), result) {
            error!(
                "Failed to send reply to server '{}' request {}: {error}",
                client.name(),
                self.id
            );
        }
    }
}

impl Editor {
    /// Apply protocol effects and reply to ordinary requests. The host presents returned actions.
    pub async fn handle_language_server_message(
        &mut self,
        call: lsp_client::Call,
        server_id: LanguageServerId,
    ) -> Option<HostAction> {
        use lsp_client::{Call, MethodCall, Notification};

        macro_rules! language_server {
            () => {
                match self.language_server_by_id(server_id) {
                    Some(language_server) => language_server,
                    None => {
                        warn!("can't find language server with id `{}`", server_id);
                        return None;
                    }
                }
            };
        }

        // Pending clients can ask for configuration before initialization completes.
        // Stopped clients must not restore state or receive deferred replies.
        if !self.language_servers.is_active(server_id) {
            return None;
        }

        match call {
            Call::Notification(lsp_client::jsonrpc::Notification { method, params, .. }) => {
                let notification = match Notification::parse(&method, params) {
                    Ok(notification) => notification,
                    Err(lsp_client::Error::Unhandled) => {
                        info!("Ignoring Unhandled notification from Language Server");
                        return None;
                    }
                    Err(err) => {
                        error!(
                            "Ignoring unknown notification from Language Server: {}",
                            err
                        );
                        return None;
                    }
                };

                match notification {
                    Notification::Initialized => {
                        self.handle_language_server_initialized(server_id);
                    }
                    Notification::PublishDiagnostics(params) => {
                        self.handle_publish_diagnostics(server_id, params);
                    }
                    Notification::ShowMessage(params) => {
                        return Some(HostAction::ShowMessage(params));
                    }
                    Notification::LogMessage(params) => {
                        log::info!("window/logMessage: {:?}", params);
                    }
                    Notification::ProgressMessage(params) => {
                        return self
                            .handle_language_server_progress(server_id, params)
                            .map(HostAction::Progress);
                    }
                    Notification::Exit => {
                        let status =
                            format!("Language server exited: {}", language_server!().name());
                        self.set_status(status);

                        self.handle_language_server_exit(server_id);
                    }
                }
            }
            Call::MethodCall(lsp_client::jsonrpc::MethodCall {
                method, params, id, ..
            }) => {
                let reply = match MethodCall::parse(&method, params) {
                    Err(lsp_client::Error::Unhandled) => {
                        error!(
                            "Language Server: Method {} not found in request {}",
                            method, id
                        );
                        Err(lsp_client::jsonrpc::Error {
                            code: lsp_client::jsonrpc::ErrorCode::MethodNotFound,
                            message: format!("Method not found: {}", method),
                            data: None,
                        })
                    }
                    Err(err) => {
                        log::error!(
                            "Language Server: Received malformed method call {} in request {}: {}",
                            method,
                            id,
                            err
                        );
                        Err(lsp_client::jsonrpc::Error {
                            code: lsp_client::jsonrpc::ErrorCode::ParseError,
                            message: format!("Malformed method call: {}", method),
                            data: None,
                        })
                    }
                    Ok(MethodCall::WorkDoneProgressCreate(params)) => {
                        self.create_language_server_progress(server_id, params.token);

                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ApplyWorkspaceEdit(params)) => {
                        let reply = DeferredReply::new(self, server_id, id);
                        self.request_workspace_edit_from_server(
                            server_id,
                            params,
                            move |editor, response| {
                                reply.complete(editor, response.map(|response| json!(response)));
                            },
                        );
                        return None;
                    }
                    Ok(MethodCall::WorkspaceFolders) => {
                        let folders = language_server!().workspace_folders();
                        Ok(if folders.is_empty() {
                            Value::Null
                        } else {
                            json!(&*folders)
                        })
                    }
                    Ok(MethodCall::WorkspaceConfiguration(params)) => {
                        Ok(json!(language_server!().configuration(&params)))
                    }
                    Ok(MethodCall::RegisterCapability(params)) => {
                        self.register_language_server_capabilities(server_id, params);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::UnregisterCapability(params)) => {
                        self.unregister_language_server_capabilities(server_id, params);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ShowDocument(_)) if !language_server!().is_initialized() => {
                        Err(jsonrpc::Error {
                            code: jsonrpc::ErrorCode::InvalidRequest,
                            message: "Server must be initialized to request document display"
                                .into(),
                            data: None,
                        })
                    }
                    Ok(MethodCall::ShowDocument(params)) => {
                        let offset_encoding = language_server!().offset_encoding();
                        return Some(HostAction::ShowDocument {
                            params,
                            offset_encoding,
                            reply: DeferredReply::new(self, server_id, id),
                        });
                    }
                    Ok(MethodCall::WorkspaceDiagnosticRefresh) => {
                        let server_id = language_server!().id();
                        crate::features::diagnostics::pull::request_all_document_diagnostics_for_language_server(
                            self,
                            server_id,
                        );

                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::SemanticTokensRefresh) => {
                        crate::features::semantic_tokens::refresh_for_language_server(
                            self, server_id,
                        );
                        Ok(Value::Null)
                    }
                    Ok(MethodCall::FoldingRangeRefresh) => {
                        self.refresh_folding_ranges(server_id);
                        Ok(Value::Null)
                    }
                    Ok(MethodCall::CodeLensRefresh) => {
                        let server_id = language_server!().id();
                        crate::features::code_lenses::refresh_for_language_server(self, server_id);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::InlayHintRefresh) => {
                        crate::features::inlay_hints::refresh_for_language_server(self, server_id);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::InlineValueRefresh) => {
                        let server_id = language_server!().id();
                        self.refresh_debugger_inline_values_for_server(server_id);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ShowMessageRequest(params)) => {
                        if let Some(actions) = params.actions.filter(|a| !a.is_empty()) {
                            return Some(HostAction::ShowMessageRequest {
                                message: params.message,
                                actions,
                                reply: DeferredReply::new(self, server_id, id),
                            });
                        } else {
                            DeferredReply::new(self, server_id, id).complete(self, Ok(Value::Null));
                            return Some(HostAction::ShowMessage(lsp::ShowMessageParams {
                                typ: params.typ,
                                message: params.message,
                            }));
                        }
                    }
                };

                DeferredReply::new(self, server_id, id).complete(self, reply);
            }
            Call::Invalid { id } => {
                log::error!("LSP invalid method call id={id:?}");
                DeferredReply::new(self, server_id, id).complete(
                    self,
                    Err(jsonrpc::Error {
                        code: jsonrpc::ErrorCode::InvalidRequest,
                        message: "Invalid request".into(),
                        data: None,
                    }),
                );
            }
        }
        None
    }
}
