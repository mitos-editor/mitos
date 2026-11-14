//! Lazy call/type hierarchy queries bound to their originating provider and document.

use anyhow::{ensure, Context as _, Result};
use futures_util::{future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId, OffsetEncoding};
use std::sync::Arc;

use super::{navigation::Location, request::SelectionRequest};
use crate::{
    action::LspActionContext, callbacks::EditorCallbackSender, DocumentId, Editor, ViewId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyKind {
    Call,
    Type,
}

impl HierarchyKind {
    fn feature(self) -> LanguageServerFeature {
        match self {
            Self::Call => LanguageServerFeature::CallHierarchy,
            Self::Type => LanguageServerFeature::TypeHierarchy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyDirection {
    Incoming,
    Outgoing,
    Supertypes,
    Subtypes,
}

impl HierarchyDirection {
    pub fn title(self) -> &'static str {
        match self {
            Self::Incoming => "Incoming calls",
            Self::Outgoing => "Outgoing calls",
            Self::Supertypes => "Supertypes",
            Self::Subtypes => "Subtypes",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RawItem {
    Call(lsp::CallHierarchyItem),
    Type(lsp_client::TypeHierarchyItem),
}

/// Keep the complete protocol item, including opaque `data`, for follow-up requests.
#[derive(Debug, Clone, PartialEq)]
pub struct HierarchyItem {
    pub name: String,
    pub detail: Option<String>,
    pub location: Location,
    server: LanguageServerId,
    raw: RawItem,
}

impl HierarchyItem {
    fn new(raw: RawItem, server: LanguageServerId, encoding: OffsetEncoding) -> Option<Self> {
        let (name, detail, uri, range) = match &raw {
            RawItem::Call(item) => (&item.name, &item.detail, &item.uri, item.selection_range),
            RawItem::Type(item) => (
                &item.symbol.name,
                &item.symbol.detail,
                &item.symbol.uri,
                item.symbol.selection_range,
            ),
        };
        Some(Self {
            name: name.clone(),
            detail: detail.clone(),
            location: Location::from_lsp(lsp::Location::new(uri.clone(), range), encoding)?,
            server,
            raw,
        })
    }

    /// Identity ignores opaque provider data and changing display metadata.
    pub fn same_symbol(&self, other: &Self) -> bool {
        self.server == other.server && self.location == other.location
    }
}

type Response = (LanguageServerId, lsp_client::Result<Vec<HierarchyItem>>);

#[derive(Clone)]
pub struct HierarchySession {
    context: LspActionContext,
    owner: EditorCallbackSender,
    document: DocumentId,
    kind: HierarchyKind,
    // A hierarchy can reference any open document. Do not reuse items after edits.
    documents: Arc<[DocumentVersion]>,
}

struct DocumentVersion {
    id: DocumentId,
    version: i32,
    uri: Option<editor_core::Uri>,
}

impl HierarchySession {
    pub fn is_current(&self, editor: &Editor) -> bool {
        editor.config().lsp.enable
            && self.owner.same_destination(editor.callback_sender())
            && editor.documents().all(|doc| {
                match self.documents.iter().find(|source| source.id == doc.id()) {
                    Some(source) => doc.version() == source.version && doc.uri() == source.uri,
                    // Newly opened buffers start at version zero. An edited buffer
                    // cannot validate cached locations even if it was opened later.
                    None => doc.version() == 0,
                }
            })
    }

    pub fn item_is_current(&self, editor: &Editor, item: &HierarchyItem) -> bool {
        self.is_current(editor)
            && self.context.is_current(editor, item.server)
            && editor.document(self.document).is_some_and(|doc| {
                doc.language_servers_with_feature(self.kind.feature())
                    .any(|server| server.id() == item.server)
            })
    }

    pub fn children(
        &self,
        editor: &Editor,
        item: &HierarchyItem,
        direction: HierarchyDirection,
    ) -> Result<BoxFuture<'static, lsp_client::Result<Vec<HierarchyItem>>>> {
        ensure!(
            self.item_is_current(editor, item),
            "Hierarchy is no longer current; reopen it"
        );
        let server = editor
            .language_server_by_id(item.server)
            .context("Hierarchy provider stopped")?;
        let id = server.id();
        let encoding = server.offset_encoding();
        let future: BoxFuture<'static, lsp_client::Result<Vec<RawItem>>> =
            match (&item.raw, direction) {
                (RawItem::Call(item), HierarchyDirection::Incoming) => {
                    let request = server.incoming_calls(item.clone());
                    async move {
                        Ok(request
                            .await?
                            .unwrap_or_default()
                            .into_iter()
                            .map(|call| RawItem::Call(call.from))
                            .collect())
                    }
                    .boxed()
                }
                (RawItem::Call(item), HierarchyDirection::Outgoing) => {
                    let request = server.outgoing_calls(item.clone());
                    async move {
                        Ok(request
                            .await?
                            .unwrap_or_default()
                            .into_iter()
                            .map(|call| RawItem::Call(call.to))
                            .collect())
                    }
                    .boxed()
                }
                (RawItem::Type(item), HierarchyDirection::Supertypes) => {
                    let request = server.supertypes(item.clone());
                    async move {
                        Ok(request
                            .await?
                            .unwrap_or_default()
                            .into_iter()
                            .map(RawItem::Type)
                            .collect())
                    }
                    .boxed()
                }
                (RawItem::Type(item), HierarchyDirection::Subtypes) => {
                    let request = server.subtypes(item.clone());
                    async move {
                        Ok(request
                            .await?
                            .unwrap_or_default()
                            .into_iter()
                            .map(RawItem::Type)
                            .collect())
                    }
                    .boxed()
                }
                _ => anyhow::bail!("Invalid hierarchy direction"),
            };
        Ok(async move {
            Ok(future
                .await?
                .into_iter()
                .filter_map(|raw| HierarchyItem::new(raw, id, encoding))
                .collect())
        }
        .boxed())
    }
}

pub struct HierarchyTask {
    request: SelectionRequest,
    session: HierarchySession,
    requests: FuturesUnordered<BoxFuture<'static, Response>>,
}

pub struct HierarchyUpdate {
    request: SelectionRequest,
    session: HierarchySession,
    results: Vec<Response>,
}

impl HierarchyTask {
    pub async fn run(mut self) -> Option<HierarchyUpdate> {
        let mut results = Vec::new();
        while let Some(result) =
            event::cancelable_future(self.requests.next(), self.request.cancel_handle()).await?
        {
            results.push(result);
        }
        Some(HierarchyUpdate {
            request: self.request,
            session: self.session,
            results,
        })
    }
}

impl HierarchyUpdate {
    pub fn resolve(self, editor: &Editor) -> Option<(HierarchySession, Vec<HierarchyItem>)> {
        if self.request.get(editor).is_none()
            || editor.tree.focus() != self.request.view_id()
            || !self.session.is_current(editor)
        {
            return None;
        }
        let mut items = Vec::new();
        for (server, result) in self.results {
            if !self.session.context.is_current(editor, server) {
                continue;
            }
            match result {
                Ok(response) => items.extend(response),
                Err(error) => log::error!("Hierarchy request failed: {error}"),
            }
        }
        Some((self.session, items))
    }
}

impl Editor {
    pub fn hierarchy(
        &mut self,
        document: DocumentId,
        view: ViewId,
        kind: HierarchyKind,
    ) -> Result<HierarchyTask> {
        ensure!(self.config().lsp.enable, "Language servers are disabled");
        ensure!(
            self.tree
                .try_get(view)
                .is_some_and(|view| view.document_id() == document),
            "Hierarchy view no longer displays the document"
        );
        let owner = self.callback_sender().clone();
        let documents = self
            .documents()
            .map(|doc| DocumentVersion {
                id: doc.id(),
                version: doc.version(),
                uri: doc.uri(),
            })
            .collect();
        let doc = self
            .document_mut(document)
            .context("Hierarchy document is closed")?;
        let requests: FuturesUnordered<_> = doc
            .language_servers_with_feature(kind.feature())
            .map(|server| {
                let id = server.id();
                let encoding = server.offset_encoding();
                let position = doc.position(view, encoding);
                let request: BoxFuture<'static, lsp_client::Result<Vec<RawItem>>> = match kind {
                    HierarchyKind::Call => {
                        let future = server.prepare_call_hierarchy(doc.identifier(), position);
                        async move {
                            Ok(future
                                .await?
                                .unwrap_or_default()
                                .into_iter()
                                .map(RawItem::Call)
                                .collect())
                        }
                        .boxed()
                    }
                    HierarchyKind::Type => {
                        let future = server.prepare_type_hierarchy(doc.identifier(), position);
                        async move {
                            Ok(future
                                .await?
                                .unwrap_or_default()
                                .into_iter()
                                .map(RawItem::Type)
                                .collect())
                        }
                        .boxed()
                    }
                };
                async move {
                    (
                        id,
                        request.await.map(|items| {
                            items
                                .into_iter()
                                .filter_map(|raw| HierarchyItem::new(raw, id, encoding))
                                .collect()
                        }),
                    )
                }
                .boxed()
            })
            .collect();
        ensure!(
            !requests.is_empty(),
            "No configured language server supports {}",
            kind.feature()
        );
        let cancel = doc
            .view_state_mut(view)
            .hierarchy
            .get_or_insert_default()
            .restart();
        Ok(HierarchyTask {
            request: SelectionRequest::new(doc, view, cancel, Vec::new()),
            session: HierarchySession {
                context: LspActionContext::new(doc),
                owner,
                document,
                kind,
                documents,
            },
            requests,
        })
    }
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    use crate::events::{ConfigDidChange, DocumentFocusLost, ModeDidChange, SelectionDidChange};
    use event::register_hook;
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        event.doc.cancel_view_request(event.view, |state| &mut state.hierarchy);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.cancel_view_request(event.view, |state| &mut state.hierarchy);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ModeDidChange<'_>| {
        for doc in event.editor.documents_mut() {
            doc.cancel_view_requests(|state| &mut state.hierarchy);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            for doc in event.editor.documents_mut() {
                doc.cancel_view_requests(|state| &mut state.hierarchy);
            }
        }
        Ok(())
    });
}
