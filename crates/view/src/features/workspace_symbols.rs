//! Workspace symbol providers captured independently of picker presentation.

use std::{collections::HashSet, sync::Weak};

use anyhow::{ensure, Context as _, Result};
use event::{register_hook, TaskHandle};
use futures_util::{future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, Client, OffsetEncoding};

use super::symbols::Symbol;
use crate::{
    callbacks::EditorCallbackSender,
    events::{ConfigDidChange, LanguageServerExited},
    DocumentId, Editor,
};

/// Providers belong to the document that opened the session, even if focus moves.
/// Frontends own query replacement and drop the old task when a query is superseded.
pub struct WorkspaceSymbolsSession {
    document: DocumentId,
    owner: EditorCallbackSender,
    servers: Vec<Weak<Client>>,
    lifetime: TaskHandle,
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
                                .is_some_and(|current| std::sync::Arc::ptr_eq(current, &server))
                    })
                })
            })
    }

    /// Start a query against the captured providers. Dropping the task cancels its LSP requests.
    pub fn query(&self, editor: &Editor, query: String) -> Result<WorkspaceSymbolsTask> {
        ensure!(
            self.is_current(editor),
            "Workspace symbol session is no longer current"
        );
        let requests = self
            .servers
            .iter()
            .filter_map(|server| {
                let server = server.upgrade()?;
                let encoding = server.offset_encoding();
                let future = server.workspace_symbols(query.clone())?;
                Some(async move { Ok(normalize_symbols(future.await?, encoding)) }.boxed())
            })
            .collect();
        Ok(WorkspaceSymbolsTask {
            lifetime: self.lifetime.clone(),
            requests,
        })
    }
}

/// Streams complete provider responses as they arrive, without borrowing the editor.
pub struct WorkspaceSymbolsTask {
    lifetime: TaskHandle,
    requests: FuturesUnordered<BoxFuture<'static, lsp_client::Result<Vec<Symbol>>>>,
}

impl WorkspaceSymbolsTask {
    pub async fn next_batch(&mut self) -> Option<lsp_client::Result<Vec<Symbol>>> {
        event::cancelable_future(self.requests.next(), &self.lifetime)
            .await
            .flatten()
    }
}

fn normalize_symbols(
    response: Option<lsp::WorkspaceSymbolResponse>,
    encoding: OffsetEncoding,
) -> Vec<Symbol> {
    match response {
        Some(lsp::WorkspaceSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .filter_map(|symbol| {
                Symbol::from_lsp(
                    symbol.name,
                    symbol.kind,
                    symbol.container_name,
                    symbol.location,
                    encoding,
                )
            })
            .collect(),
        Some(lsp::WorkspaceSymbolResponse::Nested(symbols)) => symbols
            .into_iter()
            .filter_map(|symbol| {
                let lsp::OneOf::Left(location) = symbol.location else {
                    // The client does not advertise lazy location resolution.
                    log::warn!("discarding workspace symbol without a location range");
                    return None;
                };
                Symbol::from_lsp(
                    symbol.name,
                    symbol.kind,
                    symbol.container_name,
                    location,
                    encoding,
                )
            })
            .collect(),
        None => Vec::new(),
    }
}

impl Editor {
    pub fn workspace_symbols(&mut self, document: DocumentId) -> Result<WorkspaceSymbolsSession> {
        ensure!(self.config().lsp.enable, "Language servers are disabled");
        let owner = self.callback_sender().clone();
        let doc = self
            .document_mut(document)
            .context("Workspace symbol document is closed")?;
        let mut seen = HashSet::new();
        let servers = doc
            .language_servers_with_feature(LanguageServerFeature::WorkspaceSymbols)
            .filter(|server| seen.insert(server.id()))
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
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.workspace_symbols.cancel();
            }
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_client::UriExt as _;
    use serde_json::json;

    #[test]
    fn symbol_responses_preserve_locations_and_discard_unsupported_uris() {
        let uri = lsp::Url::from_file_path(std::env::temp_dir().join("symbol.rs")).unwrap();
        let range = lsp::Range::new(lsp::Position::new(0, 3), lsp::Position::new(0, 6));
        let response = serde_json::from_value(json!([
            {"name": "valid", "kind": 12, "location": {"uri": uri, "range": range}},
            {"name": "unsupported", "kind": 12, "location": {"uri": "https://example.com/file", "range": range}}
        ])).unwrap();
        let items = normalize_symbols(Some(response), OffsetEncoding::Utf8);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "valid");
        assert_eq!(items[0].location.range, range);
        assert_eq!(items[0].location.offset_encoding, OffsetEncoding::Utf8);
    }

    #[test]
    fn modern_symbols_require_a_resolved_location() {
        let uri = lsp::Url::from_file_path(std::env::temp_dir().join("symbol.rs")).unwrap();
        let range = lsp::Range::default();
        let symbols = serde_json::from_value(json!([
            {"name": "resolved", "kind": 12, "location": {"uri": uri, "range": range}},
            {"name": "unresolved", "kind": 12, "location": {"uri": uri}}
        ]))
        .unwrap();
        let items = normalize_symbols(
            Some(lsp::WorkspaceSymbolResponse::Nested(symbols)),
            OffsetEncoding::Utf16,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "resolved");
    }
}
