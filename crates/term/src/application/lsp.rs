//! Decode LSP messages, delegate editor effects, and present interactive requests.
use super::Application;
use crate::ui;
use log::{error, info, warn};
use lsp_client::{lsp, LanguageServerId};
use serde_json::json;

impl Application {
    pub async fn handle_language_server_message(
        &mut self,
        call: lsp_client::Call,
        server_id: LanguageServerId,
    ) {
        use lsp_client::{Call, MethodCall, Notification};

        macro_rules! language_server {
            () => {
                match self.editor.language_server_by_id(server_id) {
                    Some(language_server) => language_server,
                    None => {
                        warn!("can't find language server with id `{}`", server_id);
                        return;
                    }
                }
            };
        }

        match call {
            Call::Notification(lsp_client::jsonrpc::Notification { method, params, .. }) => {
                let notification = match Notification::parse(&method, params) {
                    Ok(notification) => notification,
                    Err(lsp_client::Error::Unhandled) => {
                        info!("Ignoring Unhandled notification from Language Server");
                        return;
                    }
                    Err(err) => {
                        error!(
                            "Ignoring unknown notification from Language Server: {}",
                            err
                        );
                        return;
                    }
                };

                match notification {
                    Notification::Initialized => {
                        self.editor.handle_language_server_initialized(server_id);
                    }
                    Notification::PublishDiagnostics(params) => {
                        self.editor.handle_publish_diagnostics(server_id, params);
                    }
                    Notification::ShowMessage(params) => {
                        self.handle_show_message(params.typ, params.message);
                    }
                    Notification::LogMessage(params) => {
                        log::info!("window/logMessage: {:?}", params);
                    }
                    Notification::ProgressMessage(params) => {
                        if let Some(update) = self
                            .editor
                            .handle_language_server_progress(server_id, params)
                            && !self.compositor.has_component::<ui::Prompt>()
                            && self.editor.config().lsp.progress.enable
                        {
                            if let Some(message) = update.message {
                                self.editor.set_status(message);
                            } else if update.completed {
                                self.editor.clear_status();
                            }
                        }
                    }
                    Notification::Exit => {
                        let status =
                            format!("Language server exited: {}", language_server!().name());
                        self.editor.set_status(status);

                        self.editor.handle_language_server_exit(server_id);
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
                        self.editor
                            .create_language_server_progress(server_id, params.token);

                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ApplyWorkspaceEdit(params)) => self
                        .editor
                        .handle_workspace_edit(server_id, params)
                        .map(|response| json!(response)),
                    Ok(MethodCall::WorkspaceFolders) => {
                        Ok(json!(&*language_server!().workspace_folders().await))
                    }
                    Ok(MethodCall::WorkspaceConfiguration(params)) => {
                        Ok(json!(language_server!().configuration(&params)))
                    }
                    Ok(MethodCall::RegisterCapability(params)) => {
                        self.editor
                            .register_language_server_capabilities(server_id, params);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::UnregisterCapability(params)) => {
                        self.editor
                            .unregister_language_server_capabilities(server_id, params);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ShowDocument(params)) => {
                        let language_server = language_server!();
                        let offset_encoding = language_server.offset_encoding();

                        let result = self.handle_show_document(params, offset_encoding);
                        Ok(json!(result))
                    }
                    Ok(MethodCall::WorkspaceDiagnosticRefresh) => {
                        let server_id = language_server!().id();
                        view::features::diagnostics::pull::request_all_document_diagnostics_for_language_server(
                            &mut self.editor,
                            server_id,
                        );

                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::CodeLensRefresh) => {
                        let server_id = language_server!().id();
                        view::features::code_lenses::refresh_for_language_server(
                            &mut self.editor,
                            server_id,
                        );
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::InlineValueRefresh) => {
                        let server_id = language_server!().id();
                        self.editor
                            .refresh_debugger_inline_values_for_server(server_id);
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ShowMessageRequest(params)) => {
                        if let Some(actions) = params.actions.filter(|a| !a.is_empty()) {
                            let id = id.clone();
                            let select = ui::Select::new(
                                params.message,
                                actions,
                                (),
                                move |editor, action, event| {
                                    let reply = match event {
                                        ui::PromptEvent::Update => return,
                                        ui::PromptEvent::Validate => Some(action.clone()),
                                        ui::PromptEvent::Abort => None,
                                    };
                                    if let Some(language_server) =
                                        editor.language_server_by_id(server_id)
                                        && let Err(err) =
                                            language_server.reply(id.clone(), Ok(json!(reply)))
                                    {
                                        log::error!(
                                                "Failed to send reply to server '{}' request {id}: {err}",
                                                language_server.name()
                                            );
                                    }
                                },
                            );
                            self.compositor
                                .replace_or_push("lsp-show-message-request", select);
                            // Avoid sending a reply. The `Select` callback above sends the reply.
                            return;
                        } else {
                            self.handle_show_message(params.typ, params.message);
                            Ok(serde_json::Value::Null)
                        }
                    }
                };

                let language_server = language_server!();
                if let Err(err) = language_server.reply(id.clone(), reply) {
                    log::error!(
                        "Failed to send reply to server '{}' request {id}: {err}",
                        language_server.name()
                    );
                }
            }
            Call::Invalid { id } => log::error!("LSP invalid method call id={:?}", id),
        }
    }

    fn handle_show_message(&mut self, message_type: lsp::MessageType, message: String) {
        if self.config.load().editor.lsp.messages.enable {
            match message_type {
                lsp::MessageType::ERROR => self.editor.set_error(|| message),
                lsp::MessageType::WARNING => self.editor.set_warning(|| message),
                _ => self.editor.set_status(message),
            }
        }
    }

    fn handle_show_document(
        &mut self,
        params: lsp::ShowDocumentParams,
        offset_encoding: lsp_client::OffsetEncoding,
    ) -> lsp::ShowDocumentResult {
        if let lsp::ShowDocumentParams {
            external: Some(true),
            uri,
            ..
        } = params
        {
            self.jobs.callback(crate::open_external_url_callback(uri));
            return lsp::ShowDocumentResult { success: true };
        };

        let lsp::ShowDocumentParams {
            uri,
            selection,
            take_focus,
            ..
        } = params;

        let uri = match editor_core::Uri::try_from(uri) {
            Ok(uri) => uri,
            Err(err) => {
                log::error!("{err}");
                return lsp::ShowDocumentResult { success: false };
            }
        };
        // Determine the focus strategy based on the current UI state and LSP request:
        // 1. If there is no pop-up overlay, jump directly (Replace).
        // 2. If there is a pop-up overlay, unless the LSP forces take_focus, only load in the background (Load) to prevent interruption of input.
        // Note: We assume layer_count() == 1 means only the EditorView is present (no popups/overlays).
        let action = if self.compositor.layer_count() == 1 {
            view::editor::Action::Replace
        } else {
            match take_focus {
                Some(true) => view::editor::Action::Replace,
                _ => view::editor::Action::Load,
            }
        };

        let success =
            match self
                .editor
                .open_document_location(&uri, selection, offset_encoding, action)
            {
                Ok(_) => true,
                Err(err) => {
                    log::error!("failed to open document {uri}: {err}");
                    false
                }
            };
        lsp::ShowDocumentResult { success }
    }
}

impl ui::menu::Item for lsp::MessageActionItem {
    type Data = ();
    fn format(&self, _data: &Self::Data) -> tui::widgets::Row<'_> {
        tui::widgets::Row::new([self.title.as_str()])
    }
}
