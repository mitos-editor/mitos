//! Hover requests belong to the document and view that initiated them.

use std::{
    collections::HashSet,
    sync::{Arc, Weak},
};

use editor_core::Rope;
use event::{cancelable_future, register_hook};
use futures_util::{stream::FuturesUnordered, StreamExt};
use language_config::LanguageServerFeature;
use lsp_client::lsp;
use parking_lot::Mutex;

use super::request::SelectionRequest;
use crate::{
    callbacks::EditorCallbackSender,
    editor::Action,
    events::{ConfigDidChange, DocumentFocusLost, LanguageServerExited, SelectionDidChange},
    Document, DocumentId, Editor, ViewId,
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
    target: SelectionRequest,
}

impl Request {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.hover.shared))
            && editor.config().lsp.enable
            && self
                .target
                .get(editor)
                .is_some_and(|(view, _)| editor.tree.focus() == view.id)
    }
}

/// Request hover at the focused selection, superseding its previous request.
pub fn request(editor: &mut Editor) {
    request_impl(editor, HoverDisplay::Popup);
}

/// Request hover in a new Markdown scratch buffer in the focused view.
pub fn request_buffer(editor: &mut Editor) {
    request_impl(editor, HoverDisplay::Buffer);
}

enum HoverDisplay {
    Popup,
    Buffer,
}

fn request_impl(editor: &mut Editor, display: HoverDisplay) {
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
        target: SelectionRequest::new(doc, view.id, cancel, servers.into_iter().collect()),
    };
    let callbacks = shared.callbacks.clone();
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
                    if matches!(display, HoverDisplay::Buffer) {
                        if hovers.is_empty() {
                            editor.set_status("No hover results available.");
                        } else {
                            open_buffer(editor, hovers);
                        }
                    } else {
                        editor.handlers.hover.shared.session.lock().pending =
                            Some(HoverUpdate { request, hovers });
                    }
                    editor.redraw().request();
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

fn open_buffer(editor: &mut Editor, hovers: Vec<(String, lsp::Hover)>) {
    let mut doc = Document::from(
        Rope::from_str(&hovers_to_markdown(hovers)),
        None,
        editor.config.clone(),
        editor.syn_loader.clone(),
    );
    let loader = editor.syn_loader.load_full();
    if let Some(language) = loader.language_for_name("markdown") {
        doc.set_language(Some(loader.language(language).config().clone()), &loader);
    }
    editor.new_file_from_document(Action::Replace, doc);
}

fn hovers_to_markdown(hovers: Vec<(String, lsp::Hover)>) -> String {
    let count = hovers.len();
    let contents = hovers
        .into_iter()
        .enumerate()
        .map(|(index, (name, hover))| {
            let body = contents_to_markdown(hover.contents);
            if count > 1 {
                format!("**[{}/{}] {}**\n\n{}", index + 1, count, name, body)
            } else {
                body
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    format!("{contents}\n")
}

/// Convert a language-server hover body to Markdown for any frontend.
pub fn contents_to_markdown(contents: lsp::HoverContents) -> String {
    fn marked_string_to_markdown(contents: lsp::MarkedString) -> String {
        match contents {
            lsp::MarkedString::String(contents) => contents,
            lsp::MarkedString::LanguageString(string) => {
                if string.language == "markdown" {
                    string.value
                } else {
                    format!("```{}\n{}\n```", string.language, string.value)
                }
            }
        }
    }
    match contents {
        lsp::HoverContents::Scalar(contents) => marked_string_to_markdown(contents),
        lsp::HoverContents::Array(contents) => contents
            .into_iter()
            .map(marked_string_to_markdown)
            .collect::<Vec<_>>()
            .join("\n\n"),
        lsp::HoverContents::Markup(contents) => contents.value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_preserves_multiple_servers_and_code_blocks() {
        let hover = |contents| lsp::Hover {
            contents,
            range: None,
        };
        let markdown = hovers_to_markdown(vec![
            (
                "alpha".into(),
                hover(lsp::HoverContents::Scalar(
                    lsp::MarkedString::LanguageString(lsp::LanguageString {
                        language: "rust".into(),
                        value: "fn hello()".into(),
                    }),
                )),
            ),
            (
                "beta".into(),
                hover(lsp::HoverContents::Array(vec![
                    lsp::MarkedString::String("**Docs**".into()),
                    lsp::MarkedString::LanguageString(lsp::LanguageString {
                        language: "markdown".into(),
                        value: "More docs".into(),
                    }),
                ])),
            ),
        ]);
        assert_eq!(markdown, "**[1/2] alpha**\n\n```rust\nfn hello()\n```\n\n---\n\n**[2/2] beta**\n\n**Docs**\n\nMore docs\n");
    }
}
