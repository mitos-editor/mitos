//! Shared coordination for LSP document links results.

use std::collections::HashSet;

use editor_core::{syntax::config::LanguageServerFeature, Assoc};
use event::{cancelable_future, register_hook};
use futures_util::{stream::FuturesUnordered, StreamExt};

use super::{document_debounce::debounce_documents, lsp::DocumentRequest};
use crate::{
    callbacks::EditorCallbackSender,
    document::DocumentLink,
    events::{DocumentDidChange, DocumentDidOpen, LanguageServerExited, LanguageServerInitialized},
    DocumentId, Editor,
};

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
    let mut futures: FuturesUnordered<_> = doc
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
        let mut all_links = Vec::new();
        loop {
            match cancelable_future(futures.next(), &request.cancel).await {
                Some(Some(Ok(items))) => all_links.extend(items),
                Some(Some(Err(err))) => log::error!("document link request failed: {err}"),
                Some(None) => break,
                None => return,
            }
        }

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

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request_document_links(event.editor, event.doc, None);
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event
            .changes
            .update_positions(event.doc.document_links.cache.iter_mut().flat_map(|link| {
                std::iter::once((&mut link.start, Assoc::After))
                    .chain(std::iter::once((&mut link.end, Assoc::After)))
            }));

        if !event.ghost_transaction {
            event.doc.document_links.request.cancel();
            if let Some(handler) = &event.doc.document_links.handler {
                event::send_blocking(&handler.events, event.doc.id());
            }
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
