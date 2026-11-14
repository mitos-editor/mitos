//! Shared inbound LSP decoding, editor effects and request replies.
use crate::{features::lsp_progress::ProgressUpdate, Editor};
use log::{error, info};
use lsp_client::{jsonrpc, lsp, Client, LanguageServerId};
use serde_json::{json, Value};
use std::sync::{Arc, Weak};

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
    client: Weak<Client>,
    id: jsonrpc::Id,
}

impl DeferredReply {
    fn new(client: &Arc<Client>, id: jsonrpc::Id) -> Self {
        Self {
            client: Arc::downgrade(client),
            id,
        }
    }

    pub fn complete(self, editor: &Editor, result: Result<Value, jsonrpc::Error>) {
        if let Some(client) = self.client.upgrade() {
            reply_if_active(editor, &client, self.id, result);
        }
    }
}

fn reply_if_active(
    editor: &Editor,
    client: &Arc<Client>,
    id: jsonrpc::Id,
    result: Result<Value, jsonrpc::Error>,
) {
    // Effects can stop a server, and registry IDs may be reused across editors.
    let Some(active) = editor
        .language_servers
        .active_client(client.id())
        .filter(|active| Arc::ptr_eq(active, client))
    else {
        return;
    };
    if let Err(error) = active.reply(id.clone(), result) {
        error!(
            "Failed to send reply to server '{}' request {id}: {error}",
            active.name()
        );
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

        // Pending clients can request configuration before initialization completes.
        let client = self.language_servers.active_client(server_id).cloned()?;

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
                        let status = format!("Language server exited: {}", client.name());
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

                        Ok(Value::Null)
                    }
                    Ok(MethodCall::ApplyWorkspaceEdit(params)) => {
                        let reply = DeferredReply::new(&client, id);
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
                        let folders = client.workspace_folders();
                        Ok(if folders.is_empty() {
                            Value::Null
                        } else {
                            json!(&*folders)
                        })
                    }
                    Ok(MethodCall::WorkspaceConfiguration(params)) => {
                        Ok(json!(client.configuration(&params)))
                    }
                    Ok(MethodCall::RegisterCapability(params)) => {
                        self.register_language_server_capabilities(server_id, params);
                        Ok(Value::Null)
                    }
                    Ok(MethodCall::UnregisterCapability(params)) => {
                        self.unregister_language_server_capabilities(server_id, params);
                        Ok(Value::Null)
                    }
                    Ok(MethodCall::ShowDocument(_)) if !client.is_initialized() => {
                        Err(jsonrpc::Error {
                            code: jsonrpc::ErrorCode::InvalidRequest,
                            message: "Server must be initialized to request document display"
                                .into(),
                            data: None,
                        })
                    }
                    Ok(MethodCall::ShowDocument(params)) => {
                        let offset_encoding = client.offset_encoding();
                        return Some(HostAction::ShowDocument {
                            params,
                            offset_encoding,
                            reply: DeferredReply::new(&client, id),
                        });
                    }
                    Ok(MethodCall::WorkspaceDiagnosticRefresh) => {
                        crate::features::diagnostics::pull::request_all_document_diagnostics_for_language_server(
                            self,
                            server_id,
                        );

                        Ok(Value::Null)
                    }
                    Ok(MethodCall::TextDocumentContentRefresh(params)) => {
                        self.refresh_language_server_document(server_id, params.uri);
                        Ok(Value::Null)
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
                        crate::features::code_lenses::refresh_for_language_server(self, server_id);
                        Ok(Value::Null)
                    }
                    Ok(MethodCall::InlayHintRefresh) => {
                        crate::features::inlay_hints::refresh_for_language_server(self, server_id);
                        Ok(Value::Null)
                    }
                    Ok(MethodCall::InlineValueRefresh) => {
                        self.refresh_debugger_inline_values_for_server(server_id);
                        Ok(Value::Null)
                    }
                    Ok(MethodCall::ShowMessageRequest(params)) => {
                        if let Some(actions) = params.actions.filter(|a| !a.is_empty()) {
                            return Some(HostAction::ShowMessageRequest {
                                message: params.message,
                                actions,
                                reply: DeferredReply::new(&client, id),
                            });
                        }
                        reply_if_active(self, &client, id, Ok(Value::Null));
                        return Some(HostAction::ShowMessage(lsp::ShowMessageParams {
                            typ: params.typ,
                            message: params.message,
                        }));
                    }
                };

                reply_if_active(self, &client, id, reply);
            }
            Call::Invalid { id } => {
                log::error!("LSP invalid method call id={id:?}");
                reply_if_active(
                    self,
                    &client,
                    id,
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
