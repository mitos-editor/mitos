//! Explicit document-symbol queries, independent of breadcrumb configuration.

use anyhow::{ensure, Context as _, Result};
use futures_util::{future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, OffsetEncoding};

use crate::{
    action::LspActionContext,
    callbacks::EditorCallbackSender,
    features::{navigation::Location, request::ViewRequest},
    DocumentId, Editor, ViewId,
};

/// A symbol tree with normalized locations; frontends choose how to render hierarchy.
pub struct DocumentSymbol {
    pub symbol: lsp::SymbolInformation,
    pub location: Location,
    pub children: Vec<DocumentSymbol>,
}

type ProviderResult = (
    lsp_client::LanguageServerId,
    lsp_client::Result<Vec<DocumentSymbol>>,
);

/// Owns provider requests and the originating document/view snapshot.
pub struct DocumentSymbolsTask {
    request: ViewRequest,
    snapshot: LspActionContext,
    owner: EditorCallbackSender,
    requests: FuturesUnordered<BoxFuture<'static, ProviderResult>>,
}

/// Provider results awaiting validation on the editor's mutation path.
pub struct DocumentSymbolsUpdate {
    request: ViewRequest,
    snapshot: LspActionContext,
    owner: EditorCallbackSender,
    results: Vec<ProviderResult>,
}

impl DocumentSymbolsTask {
    pub async fn run(mut self) -> Option<DocumentSymbolsUpdate> {
        let mut results = Vec::new();
        while let Some(result) =
            event::cancelable_future(self.requests.next(), self.request.cancel_handle()).await?
        {
            results.push(result);
        }
        Some(DocumentSymbolsUpdate {
            request: self.request,
            snapshot: self.snapshot,
            owner: self.owner,
            results,
        })
    }
}

impl DocumentSymbolsUpdate {
    /// Validate after queueing, including the original view and each provider instance.
    pub fn into_symbols(self, editor: &Editor) -> Option<Vec<DocumentSymbol>> {
        if !editor.config().lsp.enable
            || !self.owner.same_destination(editor.callback_sender())
            || editor.tree.focus() != self.request.view_id()
            || self.request.get(editor).is_none()
        {
            return None;
        }
        let mut symbols = Vec::new();
        for (server, result) in self.results {
            if !self.snapshot.is_current(editor, server) {
                continue;
            }
            match result {
                Ok(mut items) => symbols.append(&mut items),
                Err(error) => log::error!("Error requesting document symbols: {error}"),
            }
        }
        Some(symbols)
    }
}

fn normalize(
    response: Option<lsp::DocumentSymbolResponse>,
    uri: &editor_core::Uri,
    encoding: OffsetEncoding,
) -> Vec<DocumentSymbol> {
    match response {
        Some(lsp::DocumentSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .filter_map(|symbol| {
                let uri = editor_core::Uri::try_from(&symbol.location.uri).ok()?;
                Some(DocumentSymbol {
                    location: Location {
                        uri,
                        range: symbol.location.range,
                        offset_encoding: encoding,
                    },
                    symbol,
                    children: Vec::new(),
                })
            })
            .collect(),
        Some(lsp::DocumentSymbolResponse::Nested(symbols)) => symbols
            .into_iter()
            .map(|symbol| {
                let children = normalize(
                    Some(lsp::DocumentSymbolResponse::Nested(
                        symbol.children.unwrap_or_default(),
                    )),
                    uri,
                    encoding,
                );
                #[allow(deprecated)]
                DocumentSymbol {
                    symbol: lsp::SymbolInformation {
                        name: symbol.name,
                        kind: symbol.kind,
                        tags: symbol.tags,
                        deprecated: symbol.deprecated,
                        location: lsp::Location::new(
                            uri.to_url().expect("document URI is an LSP URL"),
                            symbol.selection_range,
                        ),
                        container_name: None,
                    },
                    location: Location {
                        uri: uri.clone(),
                        range: symbol.selection_range,
                        offset_encoding: encoding,
                    },
                    children,
                }
            })
            .collect(),
        None => Vec::new(),
    }
}

impl Editor {
    pub fn document_symbols(
        &mut self,
        document: DocumentId,
        view: ViewId,
    ) -> Result<DocumentSymbolsTask> {
        ensure!(self.config().lsp.enable, "Language servers are disabled");
        ensure!(
            self.tree
                .try_get(view)
                .is_some_and(|view| view.doc == document),
            "Symbol view no longer displays the document"
        );
        let owner = self.callback_sender().clone();
        let doc = self
            .document_mut(document)
            .context("Symbol document is closed")?;
        let uri = doc.uri().context("Document has no URI")?;
        let mut seen = std::collections::HashSet::new();
        let requests = doc
            .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
            .filter(|server| seen.insert(server.id()))
            .filter_map(|server| {
                let future = server.document_symbols(doc.identifier())?;
                let (id, encoding, uri) = (server.id(), server.offset_encoding(), uri.clone());
                Some(
                    async move {
                        (
                            id,
                            future
                                .await
                                .map(|response| normalize(response, &uri, encoding)),
                        )
                    }
                    .boxed(),
                )
            })
            .collect::<FuturesUnordered<_>>();
        ensure!(
            !requests.is_empty(),
            "No configured language server supports document symbols"
        );
        let cancel = doc.document_symbols.picker.restart();
        Ok(DocumentSymbolsTask {
            request: ViewRequest::new(doc, view, cancel, Vec::new()),
            snapshot: LspActionContext::new(doc),
            owner,
            requests,
        })
    }
}
