//! Hover requests belong to the document and view that initiated them.

use std::sync::{Arc, Weak};

use event::{cancelable_future, register_hook};
use futures_util::{stream::FuturesUnordered, StreamExt};
use language_config::LanguageServerFeature;
use lsp_client::lsp;

use super::request::SelectionRequest;
use crate::{
    callbacks::EditorCallbackSender,
    editor::Action,
    events::{ConfigDidChange, DocumentFocusLost, LanguageServerExited, SelectionDidChange},
    Document, DocumentId, Editor, ViewId,
};

/// One editor's result destination. Request lifetimes are owned by its documents.
pub struct HoverHandler {
    callbacks: Arc<EditorCallbackSender>,
    target: Option<(DocumentId, ViewId)>,
}

impl HoverHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            callbacks: Arc::new(callbacks),
            target: None,
        }
    }
}

/// Where a frontend presents a hover response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverDisplay {
    /// A popup at the cursor.
    Popup,
    /// A new read-only buffer in the requesting view, like `goto_definition`.
    Buffer,
}

/// Open named, read-only Markdown docs in the current view, retaining the source
/// position in the jump list. The buffer remains available until explicitly closed.
pub fn open_buffer(editor: &mut Editor, text: String) -> DocumentId {
    let source = crate::current_ref!(editor).1;
    let name = source
        .path()
        .and_then(|path| path.file_name())
        .map_or_else(|| source.display_name(), |name| name.to_string_lossy());
    let doc = Document::from(
        text.into(),
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    )
    .with_readonly_name(format!("[hover: {name}]"));
    let id = editor.new_file_from_document(Action::Replace, doc);
    // If Markdown isn't configured, retain plain text.
    if let Err(err) = editor.set_document_language(id, Some("markdown")) {
        log::debug!("goto_hover buffer stays plain text: {err}");
    }
    id
}

struct Request {
    owner: Weak<EditorCallbackSender>,
    target: SelectionRequest,
    display: HoverDisplay,
}

impl Request {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.hover.callbacks))
            && editor.config().lsp.enable
            && self
                .target
                .get(editor)
                .is_some_and(|(view, _)| editor.tree.focus() == view.id)
    }
}

/// Request hover at the focused selection, superseding its previous request.
pub fn request(editor: &mut Editor) {
    request_with(editor, HoverDisplay::Popup);
}

/// Request hover for presentation as `display`, superseding the previous request
/// of either kind.
pub fn request_with(editor: &mut Editor, display: HoverDisplay) {
    let shared = editor.handlers.hover.callbacks.clone();
    editor
        .handlers
        .presentations
        .with(|pending| pending.hover = None);
    let previous = editor.handlers.hover.target.take();
    if let Some((doc, view)) = previous
        && let Some(doc) = editor.document_mut(doc)
    {
        doc.cancel_view_request(view, |state| &mut state.hover);
    }
    if !editor.config().lsp.enable {
        editor.set_error(|| "No configured language server supports hover");
        return;
    }
    let (view, doc) = crate::current!(editor);
    let cancel = doc
        .view_state_mut(view.id)
        .hover
        .get_or_insert_default()
        .restart();
    let mut servers = Vec::new();
    let mut futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::Hover)
        .inspect(|server| servers.push(server.id()))
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
    let target = (doc.id(), view.id);
    let request = Request {
        owner: Arc::downgrade(&shared),
        target: SelectionRequest::new(doc, view.id, cancel, servers),
        display,
    };
    editor.handlers.hover.target = Some(target);
    let callbacks = (*shared).clone();
    tokio::spawn(async move {
        let mut hovers = Vec::new();
        loop {
            match cancelable_future(futures.next(), request.target.cancel_handle()).await {
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
                    editor.present(|pending| pending.hover = Some(HoverUpdate { request, hovers }));
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
    pub fn display(&self) -> HoverDisplay {
        self.request.display
    }

    pub fn resolve(self, editor: &Editor) -> Option<Vec<(String, lsp::Hover)>> {
        self.request.is_current(editor).then_some(self.hovers)
    }
}

pub fn next_update(editor: &Editor) -> Option<HoverUpdate> {
    editor
        .handlers
        .presentations
        .with(|pending| pending.hover.take())
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            for doc in event.editor.documents_mut() {
                doc.cancel_view_requests(|state| &mut state.hover);
            }
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        event
            .doc
            .cancel_view_request(event.view, |state| &mut state.hover);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.cancel_view_requests(|state| &mut state.hover);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.cancel_view_requests(|state| &mut state.hover);
            }
        }
        Ok(())
    });
}
