//! Shared coordination for LSP document links results.

use event::register_hook;
use futures_util::stream::FuturesUnordered;
use language_config::LanguageServerFeature;

use crate::{
    callbacks::EditorCallbackSender,
    events::{DocumentFocusLost, SelectionDidChange},
    features::{
        debounce::debounce_documents,
        request::{collect_responses, DocumentRequest},
    },
    Document, DocumentId, Editor,
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
            refresh(editor, doc, false);
        });
        Self { callbacks, events }
    }

    pub(in crate::features) fn refresh_after_edit(doc: &mut Document, handler: Option<&Self>) {
        doc.document_links.request.cancel();
        if let Some(handler) = handler {
            event::send_blocking(&handler.events, doc.id());
        }
    }
}

/// Request document links for a specific document and cache them for navigation.
pub(in crate::features) fn refresh(editor: &mut Editor, doc_id: DocumentId, server_exited: bool) {
    let callbacks = editor.handlers.document_links.callbacks.clone();
    let registry = &editor.language_servers;
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    if server_exited {
        doc.document_links.clear_cache();
    }

    let cancel = doc.document_links.request.restart();

    let mut servers = Vec::new();
    let futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::DocumentLinks)
        .filter(|server| registry.is_ready(server.id()))
        .inspect(|server| servers.push(server.id()))
        .filter_map(|language_server| {
            let text = doc.text().clone();
            let offset_encoding = language_server.offset_encoding();
            let language_server_id = language_server.id();
            let future = language_server.text_document_document_link(doc.identifier(), None)?;

            Some(async move {
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
            })
        })
        .collect();

    if futures.is_empty() {
        return;
    }

    let request = DocumentRequest::new(doc, cancel, servers);

    tokio::spawn(async move {
        let Some(all_links) = collect_responses(futures, &request.cancel, "document link").await
        else {
            return;
        };

        callbacks
            .send(move |editor| {
                if request.is_current(editor) {
                    attach_document_links(editor, doc_id, all_links);
                }
            })
            .await;
    });
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

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        event.doc.document_links.cancel_navigation_for(event.view);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.document_links.cancel_navigation_for(event.view);
        }
        Ok(())
    });
}
