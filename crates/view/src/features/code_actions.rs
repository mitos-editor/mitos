//! Interactive code-action gathering shared by editor frontends.

use std::{collections::HashMap, sync::Arc};

use event::{cancelable_future, register_hook, TaskController, TaskHandle};
use futures_util::{future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt};
use lsp_client::lsp::{self, CodeActionOrCommand};

use super::request::SelectionRequest;
use crate::{
    action::{code_actions_for_range, Action, LspActionContext},
    callbacks::EditorCallbackSender,
    document::Mode,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentFocusLost, LanguageServerExited, ModeDidChange,
        SelectionDidChange,
    },
    Editor, ViewId,
};

#[derive(Default)]
pub(crate) struct CodeActionsState {
    requests: HashMap<ViewId, TaskController>,
}

impl CodeActionsState {
    fn restart(&mut self, view: ViewId) -> TaskHandle {
        self.requests.entry(view).or_default().restart()
    }

    pub(crate) fn remove_view(&mut self, view: ViewId) {
        self.requests.remove(&view);
    }

    pub(crate) fn clear(&mut self) {
        self.requests.clear();
    }
}

struct Context {
    owner: EditorCallbackSender,
    target: Arc<SelectionRequest>,
    mode: Mode,
}

impl Context {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner.same_destination(editor.callback_sender())
            && editor.mode() == self.mode
            && self
                .target
                .get(editor)
                .is_some_and(|(view, _)| editor.tree.focus() == view.id)
    }
}

/// An owned provider aggregation. Dropping it releases in-flight LSP requests.
pub struct CodeActionsTask {
    context: Context,
    future: BoxFuture<'static, Vec<Action>>,
}

impl CodeActionsTask {
    pub async fn run(self) -> Option<CodeActionsUpdate> {
        let actions = cancelable_future(self.future, self.context.target.cancel_handle()).await?;
        Some(CodeActionsUpdate {
            context: self.context,
            actions,
        })
    }
}

/// Results are validated when the frontend is ready to present them.
pub struct CodeActionsUpdate {
    context: Context,
    actions: Vec<Action>,
}

impl CodeActionsUpdate {
    pub fn into_actions(self, editor: &Editor) -> Option<Vec<Action>> {
        self.context.is_current(editor).then_some(self.actions)
    }
}

impl Editor {
    /// Gather actions at the focused selection from language servers, spelling, and conflicts.
    /// Disabled LSP actions are removed and higher-priority actions are returned first.
    pub fn request_code_actions(&mut self) -> CodeActionsTask {
        let owner = self.callback_sender().clone();
        let mode = self.mode();
        let lsp_enabled = self.config().lsp.enable;
        let spelling = self.spelling_actions();
        let (view, doc) = crate::current!(self);
        let action_context = LspActionContext::new(doc);
        let mut providers: FuturesUnordered<BoxFuture<'static, anyhow::Result<Vec<Action>>>> =
            FuturesUnordered::new();
        if lsp_enabled {
            for (request, server) in code_actions_for_range(
                doc,
                doc.selection(view.id).primary(),
                None,
                lsp::CodeActionTriggerKind::INVOKED,
            ) {
                let context = action_context.clone();
                providers.push(
                    async move {
                        Ok(request
                            .await?
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|action| {
                                matches!(
                                    action,
                                    CodeActionOrCommand::Command(_)
                                        | CodeActionOrCommand::CodeAction(lsp::CodeAction {
                                            disabled: None,
                                            ..
                                        })
                                )
                            })
                            .map(|action| Action::lsp(context.clone(), server, action))
                            .collect())
                    }
                    .boxed(),
                );
            }
        }
        providers.push(spelling.boxed());
        let cancel = doc.code_actions.restart(view.id);
        let context = Context {
            owner,
            target: Arc::new(SelectionRequest::new(doc, view.id, cancel, Vec::new())),
            mode,
        };
        let conflicts = super::conflicts::actions(self, context.target.clone());
        CodeActionsTask {
            context,
            future: async move {
                let mut actions = conflicts;
                while let Some(result) = providers.next().await {
                    match result {
                        Ok(items) => actions.extend(items),
                        Err(error) => log::error!("while gathering code actions: {error}"),
                    }
                }
                actions.sort_by_key(|action| std::cmp::Reverse(action.priority));
                actions
            }
            .boxed(),
        }
    }
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.code_actions.clear();
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut ModeDidChange<'_>| {
        for doc in event.editor.documents_mut() {
            doc.code_actions.clear();
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event.doc.code_actions.clear();
        Ok(())
    });
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        event.doc.code_actions.remove_view(event.view);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.code_actions.remove_view(event.view);
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        for doc in event.editor.documents_mut() {
            doc.code_actions.clear();
        }
        Ok(())
    });
}
