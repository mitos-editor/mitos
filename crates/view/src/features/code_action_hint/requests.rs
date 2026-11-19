//! Code-action availability for each document/view, independent of its presentation.

use std::{collections::HashSet, time::Duration};

use ::diagnostics::DiagnosticProvider;
use editor_core::Range;
use event::{cancelable_future, register_hook, send_blocking};
use futures_util::stream::FuturesUnordered;
use lsp_client::lsp::{CodeAction, CodeActionOrCommand, CodeActionTriggerKind};
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;

use crate::features::{debounce::debounce, request::SelectionRequest};
use crate::{
    action::code_actions_for_range_filtered,
    callbacks::EditorCallbackSender,
    events::{
        ConfigDidChange, DiagnosticsDidChange, DocumentDidChange, DocumentDidOpen,
        LanguageServerExited, LanguageServerInitialized, SelectionDidChange,
    },
    Document, DocumentId, Editor, ViewId,
};

/// Debounce queue and completion destination owned by one editor.
#[derive(Clone)]
pub struct CodeActionHintHandler {
    callbacks: EditorCallbackSender,
    events: Sender<(DocumentId, ViewId)>,
}

impl CodeActionHintHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let events = debounce(
            callbacks.clone(),
            Duration::from_millis(200),
            |editor, (doc, view)| request_code_action_hint(editor, doc, view),
        );
        Self { callbacks, events }
    }
}

fn schedule(doc: &mut Document, view: ViewId, hooks: &crate::handlers::DocumentHooks) {
    // Invalidate immediately: an old response may already be queued for publication.
    doc.clear_code_action_hints(view);
    if doc.config.load().code_action_hint()
        && !doc.is_content_readonly()
        && let Some(services) = hooks.upgrade()
        && let Some(services) = services.get()
    {
        send_blocking(&services.code_action_hint.events, (doc.id(), view));
    }
}

fn schedule_document(doc: &mut Document, hooks: &crate::handlers::DocumentHooks) {
    let views: Vec<_> = doc.view_ids().collect();
    for view in views {
        schedule(doc, view, hooks);
    }
}

fn request_code_action_hint(editor: &mut Editor, doc_id: DocumentId, view_id: ViewId) {
    if !editor.config().code_action_hint()
        || !editor
            .tree
            .try_get(view_id)
            .is_some_and(|view| view.doc == doc_id)
    {
        return;
    }
    let callbacks = editor.handlers.code_action_hint.callbacks.clone();
    let live_servers: HashSet<_> = editor
        .language_servers
        .iter_clients()
        .map(|server| server.id())
        .collect();
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    doc.ensure_view_init(view_id);
    let cancel = doc.code_action_controller(view_id).restart();
    let range = doc.selection(view_id).primary();

    // Every overlapping spelling finding offers at least "add to dictionary",
    // including buffers without an attached language server.
    let has_spelling_action = doc.diagnostics().iter().any(|diagnostic| {
        diagnostic.provider == DiagnosticProvider::Spelling
            && range.overlaps(&Range::new(diagnostic.range.start, diagnostic.range.end))
    });
    let requests = code_actions_for_range_filtered(
        doc,
        range,
        None,
        CodeActionTriggerKind::AUTOMATIC,
        Some(&live_servers),
    );
    let servers = requests.iter().map(|(_, id)| *id).collect();
    let mut futures: FuturesUnordered<_> =
        requests.into_iter().map(|(request, _)| request).collect();
    if futures.is_empty() {
        apply_code_action_hint(doc, view_id, has_spelling_action);
        return;
    }
    let request = SelectionRequest::new(doc, view_id, cancel, servers);
    tokio::spawn(async move {
        let mut available = has_spelling_action;
        loop {
            match cancelable_future(futures.next(), request.cancel_handle()).await {
                Some(Some(Ok(actions))) => {
                    available |= actions.unwrap_or_default().iter().any(|action| {
                        matches!(
                            action,
                            CodeActionOrCommand::Command(_)
                                | CodeActionOrCommand::CodeAction(CodeAction {
                                    disabled: None,
                                    ..
                                })
                        )
                    });
                }
                Some(Some(Err(err))) => log::error!("while gathering code actions: {err}"),
                Some(None) => break,
                None => return,
            }
        }
        callbacks
            .send(move |editor| {
                if request.get(editor).is_none() || !editor.config().code_action_hint() {
                    return;
                }
                let doc = editor.document_mut(doc_id).unwrap();
                apply_code_action_hint(doc, view_id, available);
            })
            .await;
    });
}

fn apply_code_action_hint(doc: &mut Document, view: ViewId, available: bool) {
    if available {
        doc.set_code_action_hints(view);
    } else {
        doc.clear_code_action_hints(view);
    }
}

pub(crate) fn register_hooks(events: &event::EventHub, hooks: crate::handlers::DocumentHooks) {
    let selection_hooks = hooks.clone();
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        schedule(event.doc, event.view, &selection_hooks);
        Ok(())
    });
    let open_hooks = hooks.clone();
    register_hook!(events => move |event: &mut DocumentDidOpen<'_>| {
        let view = event.editor.tree.focus();
        if let Some(doc) = event.editor.document_mut(event.doc) {
            schedule(doc, view, &open_hooks);
        }
        Ok(())
    });
    let change_hooks = hooks.clone();
    register_hook!(events => move |event: &mut DocumentDidChange<'_>| {
        schedule_document(event.doc, &change_hooks);

        Ok(())
    });
    let diagnostics_hooks = hooks.clone();
    register_hook!(events => move |event: &mut DiagnosticsDidChange<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            schedule_document(doc, &diagnostics_hooks);
        }
        Ok(())
    });
    let initialized_hooks = hooks.clone();
    register_hook!(events => move |event: &mut LanguageServerInitialized<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                schedule_document(doc, &initialized_hooks);
            }
        }
        Ok(())
    });
    let exited_hooks = hooks.clone();
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.clear_all_code_action_hints();
                // Debounced work runs after removal from the registry and
                // recomputes availability from the remaining providers.
                schedule_document(doc, &exited_hooks);
            }
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if event.old.code_action_hint() && !event.new.code_action_hint() {
            for doc in event.editor.documents_mut() {
                doc.clear_all_code_action_hints();
            }
        } else if !event.old.code_action_hint() && event.new.code_action_hint() {
            let view = event.editor.tree.focus();
            if let Some(doc_id) = event.editor.tree.try_get(view).map(|view| view.doc)
                && let Some(doc) = event.editor.document_mut(doc_id)
            {
                schedule(doc, view, &hooks);
            }
        }
        Ok(())
    });
}
