//! Signature-help requests and lifetime, independent of popup presentation.

use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use editor_core::{syntax::config::LanguageServerFeature, Selection};
use event::{
    cancelable_future, register_hook, send_blocking, AsyncHook, TaskController, TaskHandle,
};
use lsp_client::{lsp, LanguageServerId};
use stdx::rope::RopeSliceExt;
use tokio::{sync::mpsc::Sender, time::Instant};

use super::lsp::DocumentRequest;
use crate::{
    callbacks::EditorCallbackSender,
    document::Mode,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentFocusLost,
        LanguageServerExited, SelectionDidChange,
    },
    Document, DocumentId, Editor, ViewId,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SignatureHelpInvoked {
    Automatic,
    Manual,
}

// Preserve the existing automatic debounce; manual requests bypass it.
const TIMEOUT: Duration = Duration::from_millis(120);

#[derive(Default)]
struct Session {
    controller: TaskController,
    target: Option<(DocumentId, ViewId, Option<LanguageServerId>)>,
    pending: Option<SignatureHelpUpdate>,
}

struct Shared {
    callbacks: EditorCallbackSender,
    session: Mutex<Session>,
}

/// Owned by one editor. Documents and outstanding work retain only weak handles.
pub struct SignatureHelpHandler {
    shared: Arc<Shared>,
    events: Sender<Option<Request>>,
}

pub(crate) struct SignatureHelpTrigger {
    shared: Weak<Shared>,
    events: Sender<Option<Request>>,
}

impl SignatureHelpHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            shared: Arc::new(Shared {
                callbacks,
                session: Mutex::new(Session::default()),
            }),
            events: Debounce { request: None }.spawn(),
        }
    }

    pub(crate) fn document_trigger(&self) -> SignatureHelpTrigger {
        SignatureHelpTrigger {
            shared: Arc::downgrade(&self.shared),
            events: self.events.clone(),
        }
    }

    pub fn trigger(&self, editor: &Editor, invoked: SignatureHelpInvoked) {
        if invoked == SignatureHelpInvoked::Automatic
            && (!editor.config().lsp.auto_signature_help || editor.mode() != Mode::Insert)
        {
            return;
        }
        let Some(view) = editor.tree.try_get(editor.tree.focus) else {
            return;
        };
        let Some(doc) = editor.document(view.doc) else {
            return;
        };
        schedule(&self.shared, &self.events, doc, view.id, invoked);
    }

    /// Invalidate requests and queued results, and ask the frontend to dismiss help.
    pub fn cancel(&self) {
        close(&self.shared, &self.events);
    }
}

fn close(shared: &Arc<Shared>, events: &Sender<Option<Request>>) {
    let mut session = shared.session.lock().unwrap();
    if session.target.is_none() && session.pending.is_none() {
        return;
    }
    let cancel = session.controller.restart();
    session.target = None;
    session.pending = Some(SignatureHelpUpdate {
        owner: Arc::downgrade(shared),
        cancel,
        result: None,
    });
    drop(session);
    send_blocking(events, None);
    event::request_redraw();
}

fn schedule(
    shared: &Arc<Shared>,
    events: &Sender<Option<Request>>,
    doc: &Document,
    view: ViewId,
    invoked: SignatureHelpInvoked,
) {
    let Some(selection) = doc.selections().get(&view) else {
        return;
    };
    let server = doc
        .language_servers_with_feature(LanguageServerFeature::SignatureHelp)
        .next()
        .map(|server| server.id());
    let mut session = shared.session.lock().unwrap();
    // Cancel immediately, including responses already queued on the editor thread.
    let cancel = session.controller.restart();
    session.target = Some((doc.id(), view, server));
    session.pending = None;
    let request = Request {
        owner: Arc::downgrade(shared),
        document: DocumentRequest::new(doc, cancel, server.into_iter().collect()),
        doc: doc.id(),
        view,
        selection: selection.clone(),
        invoked,
        server,
    };
    drop(session);
    send_blocking(events, Some(request));
}

impl SignatureHelpTrigger {
    fn retrigger(&self, doc: &Document, view: Option<ViewId>) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let target = shared.session.lock().unwrap().target;
        let Some((id, target_view, _)) = target else {
            return;
        };
        if id != doc.id() || view.is_some_and(|view| view != target_view) {
            return;
        }
        if doc.config.load().lsp.auto_signature_help {
            schedule(
                &shared,
                &self.events,
                doc,
                target_view,
                SignatureHelpInvoked::Automatic,
            );
        } else {
            close(&shared, &self.events);
        }
    }
}

struct Request {
    owner: Weak<Shared>,
    document: DocumentRequest,
    doc: DocumentId,
    view: ViewId,
    selection: Selection,
    invoked: SignatureHelpInvoked,
    server: Option<LanguageServerId>,
}

impl Request {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.signature_hints.shared))
            && self.document.is_current(editor)
            && (self.invoked == SignatureHelpInvoked::Manual
                || (editor.config().lsp.auto_signature_help && editor.mode() == Mode::Insert))
            && editor.tree.focus == self.view
            && editor
                .tree
                .try_get(self.view)
                .is_some_and(|view| view.doc == self.doc)
            && editor.document(self.doc).is_some_and(|doc| {
                doc.selections().get(&self.view) == Some(&self.selection)
                    && doc
                        .language_servers_with_feature(LanguageServerFeature::SignatureHelp)
                        .next()
                        .map(|server| server.id())
                        == self.server
            })
    }

    fn run(self, editor: &mut Editor) {
        if !self.is_current(editor) {
            return;
        }
        let doc = editor.document(self.doc).unwrap();
        let future = self
            .server
            .and_then(|id| editor.language_server_by_id(id))
            .and_then(|server| {
                server.text_document_signature_help(
                    doc.identifier(),
                    doc.position(self.view, server.offset_encoding()),
                    None,
                )
            });
        let Some(future) = future else {
            if self.invoked == SignatureHelpInvoked::Manual {
                editor.set_error(|| "No configured language server supports signature-help");
            }
            // Keep the session eligible for a later edit to retry after server startup.
            return;
        };
        tokio::spawn(async move {
            let Some(result) = cancelable_future(future, &self.document.cancel).await else {
                return;
            };
            let Some(shared) = self.owner.upgrade() else {
                return;
            };
            let callbacks = shared.callbacks.clone();
            drop(shared);
            callbacks
                .send(move |editor| {
                    if !self.is_current(editor) {
                        return;
                    }
                    match result {
                        Ok(response) => self.publish(editor, response),
                        Err(error) => {
                            log::error!("signature help request failed: {error}");
                        }
                    }
                })
                .await;
        });
    }

    fn publish(self, editor: &Editor, response: Option<lsp::SignatureHelp>) {
        let response = response.filter(|response| !response.signatures.is_empty());
        let update = SignatureHelpUpdate {
            owner: self.owner.clone(),
            cancel: self.document.cancel.clone(),
            result: response.map(|response| (self, response)),
        };
        let mut session = editor
            .handlers
            .signature_hints
            .shared
            .session
            .lock()
            .unwrap();
        if update.result.is_none() {
            session.target = None;
        }
        session.pending = Some(update);
        event::request_redraw();
    }
}

struct Debounce {
    request: Option<Request>,
}

impl AsyncHook for Debounce {
    type Event = Option<Request>;

    fn handle_event(&mut self, request: Self::Event, _: Option<Instant>) -> Option<Instant> {
        self.request = request;
        match self.request.as_ref()?.invoked {
            SignatureHelpInvoked::Manual => {
                self.finish_debounce();
                None
            }
            SignatureHelpInvoked::Automatic => Some(Instant::now() + TIMEOUT),
        }
    }

    fn finish_debounce(&mut self) {
        let Some(request) = self.request.take() else {
            return;
        };
        if request.document.cancel.is_canceled() {
            return;
        }
        if let Some(shared) = request.owner.upgrade() {
            shared
                .callbacks
                .send_blocking(move |editor| request.run(editor));
        }
    }
}

/// A result awaiting presentation. Resolve it against the receiving editor before use.
pub struct SignatureHelpUpdate {
    owner: Weak<Shared>,
    cancel: TaskHandle,
    result: Option<(Request, lsp::SignatureHelp)>,
}

impl std::fmt::Debug for SignatureHelpUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignatureHelpUpdate")
            .finish_non_exhaustive()
    }
}

pub enum SignatureHelpChange {
    Show(lsp::SignatureHelp),
    Hide,
}

impl SignatureHelpUpdate {
    pub fn resolve(self, editor: &Editor) -> Option<SignatureHelpChange> {
        if self.cancel.is_canceled()
            || !self
                .owner
                .ptr_eq(&Arc::downgrade(&editor.handlers.signature_hints.shared))
        {
            return None;
        }
        match self.result {
            Some((request, response)) => request
                .is_current(editor)
                .then_some(SignatureHelpChange::Show(response)),
            None => Some(SignatureHelpChange::Hide),
        }
    }
}

/// Take the latest update for a frontend; intermediate results are superseded.
pub fn next_update(editor: &Editor) -> Option<SignatureHelpUpdate> {
    editor
        .handlers
        .signature_hints
        .shared
        .session
        .lock()
        .unwrap()
        .pending
        .take()
}

/// Forward a mode transition after applying it to the editor.
pub fn mode_changed(editor: &Editor, old: Mode) {
    if old == Mode::Insert {
        editor.handlers.signature_hints.cancel();
    } else if editor.mode() == Mode::Insert {
        editor
            .handlers
            .signature_hints
            .trigger(editor, SignatureHelpInvoked::Automatic);
    }
}

/// Interpret trigger characters using the first supported server's capabilities.
pub fn post_insert_char(editor: &Editor) {
    if !editor.config().lsp.auto_signature_help {
        return;
    }
    let Some(view) = editor.tree.try_get(editor.tree.focus) else {
        return;
    };
    let Some(doc) = editor.document(view.doc) else {
        return;
    };
    let Some(server) = doc
        .language_servers_with_feature(LanguageServerFeature::SignatureHelp)
        .next()
    else {
        return;
    };
    let triggers = server
        .capabilities()
        .signature_help_provider
        .as_ref()
        .and_then(|options| options.trigger_characters.as_ref());
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    if triggers.is_some_and(|triggers| {
        triggers
            .iter()
            .any(|trigger| text.slice(..cursor).ends_with(trigger))
    }) {
        editor
            .handlers
            .signature_hints
            .trigger(editor, SignatureHelpInvoked::Automatic);
    }
}

pub fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut DocumentDidChange<'_>| {
            if !event.ghost_transaction
                && let Some(trigger) = &event.doc.signature_help_trigger
            {
                trigger.retrigger(event.doc, None);
            }
            Ok(())
        });
        register_hook!(move |event: &mut SelectionDidChange<'_>| {
            if let Some(trigger) = &event.doc.signature_help_trigger {
                trigger.retrigger(event.doc, Some(event.view));
            }
            Ok(())
        });
        register_hook!(move |event: &mut DocumentFocusLost<'_>| {
            // Signature help follows the focused view, including splits of one document.
            event.editor.handlers.signature_hints.cancel();
            Ok(())
        });
        register_hook!(move |event: &mut DocumentDidClose<'_>| {
            cancel_document(event.editor, event.doc.id());
            Ok(())
        });
        register_hook!(move |event: &mut LanguageServerExited<'_>| {
            let handler = &event.editor.handlers.signature_hints;
            let target = handler.shared.session.lock().unwrap().target;
            if target.is_some_and(|(_, _, server)| server == Some(event.server_id)) {
                handler.cancel();
            }
            Ok(())
        });
        register_hook!(move |event: &mut ConfigDidChange<'_>| {
            if !event.new.lsp.enable
                || (event.old.lsp.auto_signature_help && !event.new.lsp.auto_signature_help)
            {
                event.editor.handlers.signature_hints.cancel();
            }
            Ok(())
        });
    });
}

fn cancel_document(editor: &Editor, doc: DocumentId) {
    let handler = &editor.handlers.signature_hints;
    let target = handler.shared.session.lock().unwrap().target;
    if target.is_some_and(|(id, _, _)| id == doc) {
        handler.cancel();
    }
}
