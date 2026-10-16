//! Hover requests belong to the document and view that initiated them.

use std::{
    collections::HashSet,
    sync::{Arc, Weak},
};

use editor_core::{syntax::config::LanguageServerFeature, Selection};
use event::{cancelable_future, register_hook};
use futures_util::{stream::FuturesUnordered, StreamExt};
use lsp_client::lsp;
use parking_lot::Mutex;

use super::request::DocumentRequest;
use crate::{
    callbacks::EditorCallbackSender,
    events::{ConfigDidChange, DocumentFocusLost, LanguageServerExited, SelectionDidChange},
    DocumentId, Editor, ViewId,
};

/// Cancellation follows the owning document and its attached views.
#[derive(Default)]
pub(crate) struct HoverState {
    requests: std::collections::HashMap<ViewId, event::TaskController>,
}

impl HoverState {
    pub(crate) fn restart(&mut self, view: ViewId) -> event::TaskHandle {
        self.requests.entry(view).or_default().restart()
    }

    pub(crate) fn remove_view(&mut self, view: ViewId) {
        self.requests.remove(&view);
    }

    pub(crate) fn clear(&mut self) {
        self.requests.clear();
    }
}

struct Shared {
    callbacks: EditorCallbackSender,
    session: Mutex<Session>,
}

#[derive(Default)]
struct Session {
    target: Option<(DocumentId, ViewId)>,
    pending: Option<HoverUpdate>,
}

/// One editor's result destination. Request lifetimes are owned by its documents.
pub struct HoverHandler {
    shared: Arc<Shared>,
}

impl HoverHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            shared: Arc::new(Shared {
                callbacks,
                session: Mutex::new(Session::default()),
            }),
        }
    }
}

struct Request {
    owner: Weak<Shared>,
    document: DocumentRequest,
    doc: DocumentId,
    view: ViewId,
    selection: Selection,
}

impl Request {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.hover.shared))
            && editor.config().lsp.enable
            && self.document.is_current(editor)
            && editor.tree.focus == self.view
            && editor
                .tree
                .try_get(self.view)
                .is_some_and(|view| view.doc == self.doc)
            && editor
                .document(self.doc)
                .is_some_and(|doc| doc.selections().get(&self.view) == Some(&self.selection))
    }
}

/// Request hover at the focused selection, superseding its previous request.
pub fn request(editor: &mut Editor) {
    let shared = editor.handlers.hover.shared.clone();
    let previous = {
        let mut session = shared.session.lock();
        session.pending = None;
        session.target.take()
    };
    if let Some((doc, view)) = previous
        && let Some(doc) = editor.document_mut(doc)
    {
        doc.hover.remove_view(view);
    }
    if !editor.config().lsp.enable {
        editor.set_error(|| "No configured language server supports hover");
        return;
    }
    let (view, doc) = crate::current!(editor);
    let cancel = doc.hover.restart(view.id);
    let mut servers = HashSet::new();
    let mut futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::Hover)
        .filter(|server| servers.insert(server.id()))
        .map(|server| {
            let name = server.name().to_owned();
            let future = server
                .text_document_hover(
                    doc.identifier(),
                    doc.position(view.id, server.offset_encoding()),
                    None,
                )
                .unwrap();
            async move { (name, future.await) }
        })
        .collect();
    if futures.is_empty() {
        editor.set_error(|| "No configured language server supports hover");
        return;
    }
    shared.session.lock().target = Some((doc.id(), view.id));
    let request = Request {
        owner: Arc::downgrade(&shared),
        document: DocumentRequest::new(doc, cancel, servers.into_iter().collect()),
        doc: doc.id(),
        view: view.id,
        selection: doc.selection(view.id).clone(),
    };
    let callbacks = shared.callbacks.clone();
    tokio::spawn(async move {
        let mut hovers = Vec::new();
        loop {
            match cancelable_future(futures.next(), &request.document.cancel).await {
                Some(Some((name, Ok(Some(hover))))) => hovers.push((name, hover)),
                Some(Some((_, Err(err)))) => log::error!("hover request failed: {err}"),
                Some(Some((_, Ok(None)))) => (),
                Some(None) => break,
                None => return,
            }
        }
        callbacks
            .send(move |editor| {
                if request.is_current(editor) {
                    editor.handlers.hover.shared.session.lock().pending =
                        Some(HoverUpdate { request, hovers });
                    event::request_redraw();
                }
            })
            .await;
    });
}

/// A queued frontend result. Validate again when the frontend presents it.
pub struct HoverUpdate {
    request: Request,
    hovers: Vec<(String, lsp::Hover)>,
}

impl std::fmt::Debug for HoverUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HoverUpdate").finish_non_exhaustive()
    }
}

impl HoverUpdate {
    pub fn resolve(self, editor: &Editor) -> Option<Vec<(String, lsp::Hover)>> {
        self.request.is_current(editor).then_some(self.hovers)
    }
}

pub fn next_update(editor: &Editor) -> Option<HoverUpdate> {
    editor.handlers.hover.shared.session.lock().pending.take()
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            for doc in event.editor.documents_mut() {
                doc.hover.clear();
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        event.doc.hover.remove_view(event.view);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.hover.clear();
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.hover.clear();
            }
        }
        Ok(())
    });
}
