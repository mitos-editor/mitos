//! Interactive code-action gathering shared by editor frontends.

use std::sync::Arc;

use event::{cancelable_future, register_hook};
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
    Editor,
};

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
    /// Gather LSP actions across selections, plus focused spelling and conflict actions.
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
            let mut requests = FuturesUnordered::new();
            let primary = doc.selection(view.id).primary_index();
            for (selection, &range) in doc.selection(view.id).iter().enumerate() {
                for (request, server) in
                    code_actions_for_range(doc, range, None, lsp::CodeActionTriggerKind::INVOKED)
                {
                    requests.push(async move { (selection, server, request.await) });
                }
            }
            providers.push(
                async move {
                    let mut responses = Vec::new();
                    while let Some((selection, server, result)) = requests.next().await {
                        match result {
                            Ok(items) => {
                                responses.push((selection, server, items.unwrap_or_default()))
                            }
                            Err(error) => log::error!("while gathering code actions: {error}"),
                        }
                    }
                    // Prefer the primary selection's representative, independent of reply order.
                    responses.sort_by_key(|(selection, ..)| (*selection != primary, *selection));
                    Ok(group_lsp_actions(action_context, primary, responses))
                }
                .boxed(),
            );
        }
        providers.push(spelling.boxed());
        let cancel = doc
            .view_state_mut(view.id)
            .code_actions
            .get_or_insert_default()
            .restart();
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

/// LSP has no cross-location action identity. Match conservatively within one server
/// by exact title and kind, and never batch ambiguous duplicates at one selection.
fn group_lsp_actions(
    context: LspActionContext,
    primary: usize,
    responses: Vec<(
        usize,
        lsp_client::LanguageServerId,
        Vec<CodeActionOrCommand>,
    )>,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut groups: Vec<(lsp_client::LanguageServerId, Vec<(usize, lsp::CodeAction)>)> = Vec::new();
    for (selection, server, items) in responses {
        for item in items {
            match item {
                CodeActionOrCommand::CodeAction(action) if action.disabled.is_some() => {}
                CodeActionOrCommand::CodeAction(action) if action.command.is_none() => {
                    if let Some((_, entries)) = groups.iter_mut().find(|(id, entries)| {
                        *id == server
                            && entries[0].1.title == action.title
                            && entries[0].1.kind == action.kind
                    }) {
                        entries.push((selection, action));
                    } else {
                        groups.push((server, vec![(selection, action)]));
                    }
                }
                // Commands can have arbitrary side effects. Preserve primary-only execution.
                item if selection == primary => {
                    actions.push(Action::lsp(context.clone(), server, item))
                }
                _ => {}
            }
        }
    }
    for (server, entries) in groups {
        let mut seen = std::collections::HashSet::new();
        if entries
            .iter()
            .any(|(selection, _)| !seen.insert(*selection))
        {
            let representative = entries[0].0;
            actions.extend(
                entries
                    .into_iter()
                    .filter(|(selection, _)| *selection == representative)
                    .map(|(_, action)| {
                        Action::lsp(
                            context.clone(),
                            server,
                            CodeActionOrCommand::CodeAction(action),
                        )
                    }),
            );
        } else {
            actions.push(Action::lsp_batch(
                context.clone(),
                server,
                entries.into_iter().map(|(_, action)| action).collect(),
            ));
        }
    }
    actions
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.cancel_view_requests(|state| &mut state.code_actions);
            }
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ModeDidChange<'_>| {
        for doc in event.editor.documents_mut() {
            doc.cancel_view_requests(|state| &mut state.code_actions);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentDidChange<'_>| {
        event
            .doc
            .cancel_view_requests(|state| &mut state.code_actions);
        Ok(())
    });
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        event
            .doc
            .cancel_view_request(event.view, |state| &mut state.code_actions);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.cancel_view_request(event.view, |state| &mut state.code_actions);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        for doc in event.editor.documents_mut() {
            doc.cancel_view_requests(|state| &mut state.code_actions);
        }
        Ok(())
    });
}
