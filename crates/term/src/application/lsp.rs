//! Present host actions returned by the editor’s inbound LSP dispatcher.
use super::App;
use crate::ui;
use lsp_client::UriExt as _;
use lsp_client::{lsp, LanguageServerId};
use serde_json::json;

impl<B: tui::backend::Backend + tui::backend::BackendExt> App<B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    pub async fn handle_language_server_message(
        &mut self,
        call: lsp_client::Call,
        server_id: LanguageServerId,
    ) {
        use view::handlers::lsp::HostAction;
        match self
            .editor
            .handle_language_server_message(call, server_id)
            .await
        {
            Some(HostAction::Progress(update)) => {
                if !self.compositor.has_component::<ui::Prompt>()
                    && self.editor.config().lsp.progress.enable
                {
                    if let Some(message) = update.message {
                        self.editor.set_status(message);
                    } else if update.completed {
                        self.editor.clear_status();
                    }
                }
            }
            Some(HostAction::ShowMessage(params)) => {
                self.handle_show_message(params.typ, params.message);
            }
            Some(HostAction::ShowMessageRequest {
                message,
                actions,
                reply,
            }) => {
                let reply = std::cell::Cell::new(Some(reply));
                let select = ui::Select::new(message, actions, (), move |editor, action, event| {
                    let result = match event {
                        ui::PromptEvent::Update => return,
                        ui::PromptEvent::Validate => Some(action.clone()),
                        ui::PromptEvent::Abort => None,
                    };
                    if let Some(reply) = reply.take() {
                        reply.complete(editor, Ok(json!(result)));
                    }
                });
                self.compositor
                    .replace_or_push("lsp-show-message-request", select);
            }
            Some(HostAction::ShowDocument {
                params,
                offset_encoding,
                reply,
            }) => {
                if params.external != Some(true)
                    && !params
                        .uri
                        .as_str()
                        .split_once(':')
                        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
                {
                    let action = if params.take_focus == Some(false) {
                        view::editor::Action::Load
                    } else {
                        self.show_document_action(params.take_focus)
                    };
                    self.editor.request_language_server_document(
                        server_id,
                        params.uri,
                        params.selection,
                        action,
                        move |editor, result| {
                            reply.complete(
                                editor,
                                Ok(json!(lsp::ShowDocumentResult {
                                    success: result.is_ok(),
                                })),
                            );
                        },
                    );
                } else {
                    let result = self.handle_show_document(params, offset_encoding);
                    reply.complete(&self.editor, Ok(json!(result)));
                }
            }
            None => {}
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
            let Ok(uri) = uri.to_editor_url() else {
                return lsp::ShowDocumentResult { success: false };
            };
            self.jobs.callback(crate::open_external_url_callback(uri));
            return lsp::ShowDocumentResult { success: true };
        };

        let lsp::ShowDocumentParams {
            uri,
            selection,
            take_focus,
            ..
        } = params;

        let uri = match uri.to_editor_uri() {
            Ok(uri) => uri,
            Err(err) => {
                log::error!("{err}");
                return lsp::ShowDocumentResult { success: false };
            }
        };
        let action = self.show_document_action(take_focus);

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

    fn show_document_action(&self, take_focus: Option<bool>) -> view::editor::Action {
        if self.compositor.layer_count() == 1 || take_focus == Some(true) {
            view::editor::Action::Replace
        } else {
            view::editor::Action::Load
        }
    }
}

impl ui::menu::Item for lsp::MessageActionItem {
    type Data = ();
    fn format(&self, _data: &Self::Data) -> tui::widgets::Row<'_> {
        tui::widgets::Row::new([self.title.as_str()])
    }
}
