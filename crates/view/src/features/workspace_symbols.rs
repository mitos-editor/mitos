//! Workspace symbol queries and lazy locations owned by their original provider.

use std::sync::{Arc, Mutex, OnceLock, Weak};

use anyhow::{ensure, Context as _, Result};
use event::{cancelable_future, register_hook, TaskController, TaskHandle};
use futures_util::{
    future::BoxFuture,
    stream::{self, BoxStream, SelectAll},
    FutureExt, StreamExt,
};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, Client, OffsetEncoding, RequestUpdate, UriExt as _};

use super::{navigation::Location, request::DocumentRequest};
use crate::{
    callbacks::EditorCallbackSender,
    events::{ConfigDidChange, LanguageServerExited},
    DocumentId, Editor,
};

/// Providers belong to the document that opened the session, even if focus moves.
/// Starting a new query cancels requests and lazy resolutions for the old query.
pub struct WorkspaceSymbolsSession {
    document: DocumentId,
    owner: EditorCallbackSender,
    servers: Vec<Weak<Client>>,
    lifetime: TaskHandle,
    query: Mutex<TaskController>,
}

impl WorkspaceSymbolsSession {
    pub fn is_current(&self, editor: &Editor) -> bool {
        !self.lifetime.is_canceled()
            && self.owner.same_destination(editor.callback_sender())
            && editor.config().lsp.enable
            && editor.document(self.document).is_some_and(|doc| {
                self.servers.iter().all(|server| {
                    server.upgrade().is_some_and(|server| {
                        doc.supports_language_server(server.id())
                            && editor
                                .language_servers
                                .get_by_id(server.id())
                                .is_some_and(|current| Arc::ptr_eq(current, &server))
                    })
                })
            })
    }

    pub fn query(&self, editor: &Editor, query: String) -> Result<WorkspaceSymbolsTask> {
        ensure!(
            self.is_current(editor),
            "Workspace symbol session is no longer current"
        );
        let cancel = self.query.lock().unwrap().restart();
        let doc = editor.document(self.document).unwrap();
        let requests = self
            .servers
            .iter()
            .filter_map(|server| {
                let client = server.upgrade()?;
                let stream = client.workspace_symbols_stream(query.clone())?;
                let mut provider = TaskController::default();
                let valid = provider.restart();
                let request = Arc::new(Query {
                    owner: self.owner.clone(),
                    target: DocumentRequest::new(doc, cancel.clone(), vec![client.id()]),
                    lifetime: self.lifetime.clone(),
                    server: server.clone(),
                    encoding: client.offset_encoding(),
                    text: query.clone(),
                    provider: Mutex::new(provider),
                    valid,
                });
                let request = PendingQuery {
                    query: request,
                    completed: false,
                };
                Some(
                    stream::unfold(Some((stream, request)), |state| async move {
                        let (mut stream, mut request) = state?;
                        let update = stream.next().await?;
                        let (response, next) = match update {
                            RequestUpdate::Partial(Ok(batch)) => (Ok(Some(batch)), Some(stream)),
                            RequestUpdate::Partial(Err(error)) => (Err(error), None),
                            RequestUpdate::Response(response) => {
                                (response.map(Option::flatten), None)
                            }
                        };
                        let response =
                            response.map(|batch| normalize_symbols(batch, request.query.clone()));
                        request.completed = next.is_none() && response.is_ok();
                        Some((response, next.map(|stream| (stream, request))))
                    })
                    .boxed(),
                )
            })
            .collect();
        Ok(WorkspaceSymbolsTask {
            lifetime: self.lifetime.clone(),
            cancel,
            requests,
        })
    }
}

struct Query {
    owner: EditorCallbackSender,
    target: DocumentRequest,
    lifetime: TaskHandle,
    server: Weak<Client>,
    encoding: OffsetEncoding,
    text: String,
    provider: Mutex<TaskController>,
    valid: TaskHandle,
}

/// Provisional rows and lazy resolutions expire on failure or local cancellation.
/// A successful final response releases the guard while keeping its rows usable.
struct PendingQuery {
    query: Arc<Query>,
    completed: bool,
}

impl Drop for PendingQuery {
    fn drop(&mut self) {
        if !self.completed {
            self.query.provider.lock().unwrap().cancel();
        }
    }
}

impl Query {
    fn location(&self, location: lsp::Location) -> Option<Location> {
        Location::from_lsp(location, self.encoding)
    }

    fn is_current(&self, editor: &Editor) -> bool {
        !self.lifetime.is_canceled()
            && !self.valid.is_canceled()
            && self.owner.same_destination(editor.callback_sender())
            && editor.config().lsp.enable
            && self.target.is_current(editor)
            && self.server.upgrade().is_some_and(|server| {
                editor
                    .language_server_by_id(server.id())
                    .is_some_and(|current| std::ptr::eq(current, server.as_ref()))
            })
    }
}

/// A workspace item retains its wire data until the user needs its location.
/// Clones share the location cache, but only a validated response can populate it.
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: lsp::SymbolKind,
    pub container_name: Option<String>,
    pub uri: editor_core::Uri,
    location: OnceLock<Location>,
    wire: lsp::WorkspaceSymbol,
    query: Arc<Query>,
}

impl PartialEq for WorkspaceSymbol {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl WorkspaceSymbol {
    pub fn location<'a>(&'a self, editor: &Editor) -> Option<&'a Location> {
        self.is_current(editor)
            .then(|| self.location.get())
            .flatten()
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.query.is_current(editor)
    }

    pub fn query(&self) -> &str {
        &self.query.text
    }

    pub fn provider(&self) -> Option<lsp_client::LanguageServerId> {
        self.query.server.upgrade().map(|server| server.id())
    }

    pub fn resolve(self: &Arc<Self>, editor: &Editor) -> Result<WorkspaceSymbolResolveTask> {
        ensure!(
            self.is_current(editor),
            "Workspace symbol is no longer current"
        );
        let future = if self.location.get().is_some() {
            None
        } else {
            let server = self
                .query
                .server
                .upgrade()
                .context("Symbol provider stopped")?;
            let symbol = self.wire.clone();
            Some(
                server
                    .resolve_workspace_symbol(symbol)
                    .context("Symbol provider does not support resolution")?
                    .boxed(),
            )
        };
        Ok(WorkspaceSymbolResolveTask {
            symbol: self.clone(),
            future,
        })
    }
}

pub struct WorkspaceSymbolResolveTask {
    symbol: Arc<WorkspaceSymbol>,
    future: Option<BoxFuture<'static, lsp_client::Result<lsp::WorkspaceSymbol>>>,
}

impl WorkspaceSymbolResolveTask {
    pub async fn run(self) -> Result<Option<WorkspaceSymbolResolution>> {
        let location = match self.future {
            Some(future) => {
                let response = cancelable_future(
                    cancelable_future(
                        cancelable_future(future, &self.symbol.query.target.cancel),
                        &self.symbol.query.lifetime,
                    ),
                    &self.symbol.query.valid,
                )
                .await
                .flatten()
                .flatten();
                let Some(response) = response else {
                    return Ok(None);
                };
                let lsp::OneOf::Left(location) = response?.location else {
                    anyhow::bail!("Symbol provider returned no location range");
                };
                self.symbol
                    .query
                    .location(location)
                    .context("Symbol provider returned an unsupported location")?
            }
            None => self.symbol.location.get().unwrap().clone(),
        };
        Ok(Some(WorkspaceSymbolResolution {
            symbol: self.symbol,
            location,
        }))
    }
}

/// A queued resolution rechecks its query, document, and server before publication.
pub struct WorkspaceSymbolResolution {
    symbol: Arc<WorkspaceSymbol>,
    location: Location,
}

impl WorkspaceSymbolResolution {
    pub fn publish(self, editor: &Editor) -> Option<Arc<WorkspaceSymbol>> {
        if !self.symbol.is_current(editor) {
            return None;
        }
        let _ = self.symbol.location.set(self.location);
        Some(self.symbol)
    }
}

pub struct WorkspaceSymbolsTask {
    lifetime: TaskHandle,
    cancel: TaskHandle,
    requests: SelectAll<BoxStream<'static, lsp_client::Result<Vec<Arc<WorkspaceSymbol>>>>>,
}

impl WorkspaceSymbolsTask {
    pub async fn next_batch(&mut self) -> Option<lsp_client::Result<Vec<Arc<WorkspaceSymbol>>>> {
        cancelable_future(
            cancelable_future(self.requests.next(), &self.cancel),
            &self.lifetime,
        )
        .await
        .flatten()
        .flatten()
    }
}

fn normalize_symbols(
    response: Option<lsp::WorkspaceSymbolResponse>,
    query: Arc<Query>,
) -> Vec<Arc<WorkspaceSymbol>> {
    let symbols = match response {
        Some(lsp::WorkspaceSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|symbol| lsp::WorkspaceSymbol {
                name: symbol.name,
                kind: symbol.kind,
                tags: symbol.tags,
                container_name: symbol.container_name,
                location: lsp::OneOf::Left(symbol.location),
                data: None,
            })
            .collect(),
        Some(lsp::WorkspaceSymbolResponse::Nested(symbols)) => symbols,
        None => return Vec::new(),
    };
    symbols
        .into_iter()
        .filter_map(|symbol| {
            let (uri, location) = match &symbol.location {
                lsp::OneOf::Left(location) => {
                    let location = query.location(location.clone())?;
                    (location.uri.clone(), Some(location))
                }
                lsp::OneOf::Right(location) => (location.uri.to_editor_uri().ok()?, None),
            };
            let cache = OnceLock::new();
            if let Some(location) = location {
                let _ = cache.set(location);
            }
            Some(Arc::new(WorkspaceSymbol {
                name: symbol.name.clone(),
                kind: symbol.kind,
                container_name: symbol.container_name.clone(),
                uri,
                location: cache,
                wire: symbol,
                query: query.clone(),
            }))
        })
        .collect()
}

impl Editor {
    pub fn workspace_symbols(&mut self, document: DocumentId) -> Result<WorkspaceSymbolsSession> {
        ensure!(self.config().lsp.enable, "Language servers are disabled");
        let owner = self.callback_sender().clone();
        let doc = self
            .document_mut(document)
            .context("Workspace symbol document is closed")?;
        let servers = doc
            .language_servers_with_feature(LanguageServerFeature::WorkspaceSymbols)
            .filter_map(|server| doc.language_servers.get(server.name()))
            .map(|attachment| std::sync::Arc::downgrade(&attachment.client))
            .collect::<Vec<_>>();
        ensure!(
            !servers.is_empty(),
            "No configured language server supports workspace symbols"
        );
        Ok(WorkspaceSymbolsSession {
            document,
            owner,
            servers,
            lifetime: doc.workspace_symbols.restart(),
            query: Mutex::new(TaskController::new()),
        })
    }
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            for doc in event.editor.documents_mut() {
                doc.workspace_symbols.cancel();
            }
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.lsp_documents_mut(event.server_id) {
            doc.workspace_symbols.cancel();
        }
        Ok(())
    });
}
