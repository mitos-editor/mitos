//! Shared coordination for LSP document links results.

use crate::features::request::{collect_responses, DocumentRequest};
use std::collections::HashSet;

use event::register_hook;
use futures_util::stream::FuturesUnordered;
use language_config::LanguageServerFeature;

use crate::{
    callbacks::EditorCallbackSender,
    events::{
        DocumentDidChange, DocumentDidOpen, DocumentFocusLost, DocumentLanguageDidChange,
        LanguageServerExited, LanguageServerInitialized, SelectionDidChange,
    },
    features::debounce::debounce_documents,
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

    // Exit hooks run before the server is removed from the registry.
    let mut seen_language_servers = HashSet::new();
    let futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::DocumentLinks)
        .filter(|ls| Some(ls.id()) != exited_server)
        .filter(|ls| seen_language_servers.insert(ls.id()))
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

    let request = DocumentRequest::new(doc, cancel, seen_language_servers.into_iter().collect());

    tokio::spawn(async move {
        let all_links =
            collect_responses(futures, &request.cancel, "document link request failed").await?;
        callbacks
            .send(move |editor| {
                if request.is_current(editor) {
                    attach_document_links(editor, doc_id, all_links);
                }
            })
            .await;
        Some(())
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

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentLanguageDidChange<'_>| {
        request_document_links(event.editor, event.doc, None);
        Ok(())
    });
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
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request_document_links(event.editor, event.doc, None);
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event.doc.document_links.request.cancel();
        if let Some(handler) = &event.doc.document_links.handler {
            event::send_blocking(&handler.events, event.doc.id());
        }

        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let doc_ids: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();

        for doc_id in doc_ids {
            request_document_links(event.editor, doc_id, None);
        }

        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.document_links.clear_cache();
            }
        }

        let doc_ids: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();

        for doc_id in doc_ids {
            request_document_links(event.editor, doc_id, Some(event.server_id));
        }

        Ok(())
    });
}
