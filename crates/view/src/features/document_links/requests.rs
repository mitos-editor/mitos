//! Shared coordination for LSP document links results.

use event::register_hook;
use language_config::LanguageServerFeature;

use crate::{
    callbacks::EditorCallbackSender,
    events::{DocumentFocusLost, SelectionDidChange},
    features::{
        debounce::debounce_documents,
        request::{collect_document_results, document_providers, register_document_refresh_hooks},
    },
    DocumentId, Editor,
};

use super::DocumentLink;

/// Scheduling handle attached to documents owned by this editor.
#[derive(Clone)]
pub struct DocumentLinksHandler {
    callbacks: EditorCallbackSender,
    events: tokio::sync::mpsc::Sender<DocumentId>,
}

impl DocumentLinksHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let events = debounce_documents(callbacks.clone(), |editor, doc| {
            request_document_links(editor, doc, None);
        });
        Self { callbacks, events }
    }
}

/// Request document links for a specific document and cache them for navigation.
fn request_document_links(
    editor: &mut Editor,
    doc_id: DocumentId,
    exited_server: Option<lsp_client::LanguageServerId>,
) {
    let callbacks = editor.handlers.document_links.callbacks.clone();
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    let cancel = doc.document_links.request.restart();

    let requests = document_providers(doc, LanguageServerFeature::DocumentLinks, exited_server)
        .filter_map(|language_server| {
            let text = doc.text().clone();
            let offset_encoding = language_server.offset_encoding();
            let language_server_id = language_server.id();
            let future = language_server.text_document_document_link(doc.identifier(), None)?;

            Some((language_server_id, async move {
                let links = future.await?.unwrap_or_default();
                let links: Vec<_> = links
                    .into_iter()
                    .filter_map(|link| {
                        let start = lsp_client::util::lsp_pos_to_pos(
                            &text,
                            link.range.start,
                            offset_encoding,
                        )?;
                        let end = lsp_client::util::lsp_pos_to_pos(
                            &text,
                            link.range.end,
                            offset_encoding,
                        )?;
                        if start > end {
                            return None;
                        }
                        Some(DocumentLink {
                            start,
                            end,
                            link,
                            language_server_id,
                        })
                    })
                    .collect();
                anyhow::Ok(links)
            }))
        });
    collect_document_results(
        doc,
        cancel,
        callbacks,
        requests,
        "document link",
        attach_document_links,
    );
}

fn attach_document_links(editor: &mut Editor, doc_id: DocumentId, mut links: Vec<DocumentLink>) {
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };

    if links.is_empty() {
        doc.document_links.clear_cache();
        return;
    }

    links.sort_by_key(|link| (link.start, link.end));
    doc.document_links.cache = links;
}

pub(crate) fn register_hooks() {
    register_document_refresh_hooks(
        request_document_links,
        |doc| {
            doc.document_links.request.cancel();
            if let Some(handler) = &doc.document_links.handler {
                event::send_blocking(&handler.events, doc.id());
            }
        },
        |doc| doc.document_links.clear_cache(),
    );
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        event.doc.document_links.cancel_navigation_for(event.view);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.document_links.cancel_navigation_for(event.view);
        }
        Ok(())
    });
}
