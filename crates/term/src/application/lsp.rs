//! Present the host actions produced by shared LSP dispatch.
use super::App;
use crate::ui;
use lsp_client::{lsp, LanguageServerId, UriExt as _};
use serde_json::json;
use view::handlers::lsp::HostAction;

impl<B: tui::backend::Backend + tui::backend::BackendExt> App<B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    pub async fn handle_language_server_message(
        &mut self,
        call: lsp_client::Call,
        server_id: LanguageServerId,
    ) {
        let Some(action) = self
            .editor
            .handle_language_server_message(call, server_id)
            .await
        else {
            return;
        };
        match action {
            HostAction::Progress(update) => {
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
            HostAction::ShowDocument {
                params,
                offset_encoding,
                reply,
            } => {
                let result = self.handle_show_document(params, offset_encoding);
                reply.complete(&self.editor, Ok(json!(result)));
            }
            HostAction::ShowMessage {
                message,
                actions,
                reply,
            } => {
                let reply = std::cell::Cell::new(Some(reply));
                let select = ui::Select::new(message, actions, (), move |editor, action, event| {
                    let choice = match event {
                        ui::PromptEvent::Update => return,
                        ui::PromptEvent::Validate => Some(action.clone()),
                        ui::PromptEvent::Abort => None,
                    };
                    if let Some(reply) = reply.take() {
                        reply.complete(editor, Ok(json!(choice)));
                    }
                });
                self.compositor
                    .replace_or_push("lsp-show-message-request", select);
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
