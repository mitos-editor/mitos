//! Snapshot validation shared by document features.

use std::{collections::HashSet, future::Future};

use editor_core::{Selection, Uri};
use event::{cancelable_future, register_hook};
use futures_util::{stream::FuturesUnordered, StreamExt};
use language_config::LanguageServerFeature;
use lsp_client::{Client, LanguageServerId};

use crate::{
    callbacks::EditorCallbackSender,
    events::{
        DocumentDidChange, DocumentDidOpen, DocumentLanguageDidChange, LanguageServerExited,
        LanguageServerInitialized,
    },
    Document, DocumentId, Editor, View, ViewId,
};

/// A provider may appear under multiple configured names. Exit hooks run before
/// registry removal, so replacement requests must also exclude the exiting server.
pub(crate) fn document_providers(
    doc: &Document,
    feature: LanguageServerFeature,
    exited: Option<LanguageServerId>,
) -> impl Iterator<Item = &Client> {
    let mut seen = HashSet::new();
    doc.language_servers_with_feature(feature)
        .filter(move |server| Some(server.id()) != exited && seen.insert(server.id()))
}

/// Collect successful provider batches without losing other providers' results
/// on failure. Both the wait and queued publication honor the request lifetime.
pub(crate) fn collect_document_results<T, F>(
    doc: &Document,
    cancel: event::TaskHandle,
    callbacks: EditorCallbackSender,
    requests: impl Iterator<Item = (LanguageServerId, F)>,
    feature: &'static str,
    apply: impl FnOnce(&mut Editor, DocumentId, Vec<T>) + Send + 'static,
) -> bool
where
    T: Send + 'static,
    F: Future<Output = anyhow::Result<Vec<T>>> + Send + 'static,
{
    let (servers, mut futures): (Vec<_>, FuturesUnordered<_>) = requests.unzip();
    if futures.is_empty() {
        return false;
    }
    let request = DocumentRequest::new(doc, cancel, servers);
    let doc = doc.id();
    request.publish(
        callbacks,
        async move {
            let mut items = Vec::new();
            while let Some(result) = futures.next().await {
                match result {
                    Ok(batch) => items.extend(batch),
                    Err(err) => log::error!("{feature} request failed: {err}"),
                }
            }
            Some(items)
        },
        move |editor, items| apply(editor, doc, items),
    );
    true
}

/// Lifecycle shared by document-wide features refreshed after open, language
/// changes, edits, and server changes. Selection and configuration policy remain
/// in each feature; `changed` owns its cancellation and debounce behavior.
pub(crate) fn register_document_refresh_hooks(
    request: fn(&mut Editor, DocumentId, Option<LanguageServerId>),
    changed: fn(&mut Document),
    clear: fn(&mut Document),
) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request(event.editor, event.doc, None);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentLanguageDidChange<'_>| {
        request(event.editor, event.doc, None);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        changed(event.doc);
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let docs: Vec<_> = event.editor.documents().map(Document::id).collect();
        for doc in docs {
            request(event.editor, doc, None);
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                clear(doc);
            }
        }
        let docs: Vec<_> = event.editor.documents().map(Document::id).collect();
        for doc in docs {
            request(event.editor, doc, Some(event.server_id));
        }
        Ok(())
    });
}

/// Snapshot identity retained until a queued document-feature response is applied.
/// Cancellation while waiting on the server alone does not protect queued callbacks.
pub(crate) struct DocumentRequest {
    pub(crate) cancel: event::TaskHandle,
    doc: DocumentId,
    buffer: buffer::Snapshot,
    uri: Option<Uri>,
    servers: Vec<LanguageServerId>,
}

impl DocumentRequest {
    /// Cancellation covers the computation and its queued result. Features retain
    /// their own error handling and any selection, mode, or configuration checks.
    pub(crate) fn publish<T: Send + 'static>(
        self,
        callbacks: EditorCallbackSender,
        result: impl Future<Output = Option<T>> + Send + 'static,
        apply: impl FnOnce(&mut Editor, T) + Send + 'static,
    ) {
        tokio::spawn(async move {
            let Some(Some(result)) = cancelable_future(result, &self.cancel).await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if self.is_current(editor) {
                        apply(editor, result);
                    }
                })
                .await;
        });
    }

    pub(crate) fn new(
        doc: &Document,
        cancel: event::TaskHandle,
        servers: Vec<LanguageServerId>,
    ) -> Self {
        Self {
            cancel,
            doc: doc.id(),
            buffer: doc.snapshot(),
            uri: doc.uri(),
            servers,
        }
    }

    pub(crate) fn is_current(&self, editor: &Editor) -> bool {
        self.get(editor).is_some()
    }

    fn get<'a>(&self, editor: &'a Editor) -> Option<&'a Document> {
        if self.cancel.is_canceled() {
            return None;
        }
        let doc = editor.document(self.doc)?;
        (self.buffer.is_current(&doc.buffer)
            && doc.uri() == self.uri
            && self.servers.iter().all(|&id| {
                editor.language_server_by_id(id).is_some() && doc.supports_language_server(id)
            }))
        .then_some(doc)
    }
}

/// A document snapshot bound to one view, including views that are not focused.
pub(crate) struct ViewRequest {
    document: DocumentRequest,
    view: ViewId,
}

impl ViewRequest {
    pub(crate) fn new(
        doc: &Document,
        view: ViewId,
        cancel: event::TaskHandle,
        servers: Vec<LanguageServerId>,
    ) -> Self {
        Self {
            document: DocumentRequest::new(doc, cancel, servers),
            view,
        }
    }

    pub(crate) fn document_id(&self) -> DocumentId {
        self.document.doc
    }

    pub(crate) fn view_id(&self) -> ViewId {
        self.view
    }

    pub(crate) fn cancel_handle(&self) -> &event::TaskHandle {
        &self.document.cancel
    }

    /// Return the bound view and document while the snapshot is current.
    /// Focus, mode, and feature-specific configuration remain the caller's policy.
    pub(crate) fn get<'a>(&self, editor: &'a Editor) -> Option<(&'a View, &'a Document)> {
        let doc = self.document.get(editor)?;
        let view = editor.tree.try_get(self.view)?;
        (view.doc == doc.id()).then_some((view, doc))
    }
}

/// A view-bound snapshot that also requires the original selection.
/// Completion sessions that map edits through savepoints have a different lifetime.
pub(crate) struct SelectionRequest {
    target: ViewRequest,
    selection: Selection,
}

impl SelectionRequest {
    pub(crate) fn new(
        doc: &Document,
        view: ViewId,
        cancel: event::TaskHandle,
        servers: Vec<LanguageServerId>,
    ) -> Self {
        Self {
            target: ViewRequest::new(doc, view, cancel, servers),
            selection: doc.selection(view).clone(),
        }
    }

    pub(crate) fn document_id(&self) -> DocumentId {
        self.target.document_id()
    }

    pub(crate) fn view_id(&self) -> ViewId {
        self.target.view_id()
    }

    pub(crate) fn cancel_handle(&self) -> &event::TaskHandle {
        self.target.cancel_handle()
    }

    pub(crate) fn get<'a>(&self, editor: &'a Editor) -> Option<(&'a View, &'a Document)> {
        let (view, doc) = self.target.get(editor)?;
        (doc.try_selection(view.id) == Some(&self.selection)).then_some((view, doc))
    }
}
