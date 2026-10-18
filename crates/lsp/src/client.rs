//! Shared client state and JSON-RPC request, notification, and response dispatch.

mod document;
mod file_operations;
mod process;
mod requests;
mod workspace;

use crate::{
    file_operations::FileOperationsInterest,
    jsonrpc, lsp,
    transport::{Payload, PendingRequests},
    Error, LanguageServerId, OffsetEncoding, Result,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::{
    future::Future,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};
use tokio::{
    process::Child,
    sync::{mpsc::UnboundedSender, Notify, OnceCell},
};

#[derive(Debug)]
/// One language-server process and its negotiated workspace state.
///
/// Construction returns a receiver for server-originated calls. The client
/// handles responses to its own requests internally, but the editor event loop
/// must keep polling server requests and notifications.
///
/// Requests are queued when their futures are created. Dropping a request future
/// or reaching its configured deadline cancels local tracking and asks the server
/// to stop any work already sent. Cancellation is best effort on the server.
pub struct Client {
    id: LanguageServerId,
    name: String,
    _process: Child,
    server_tx: UnboundedSender<Payload>,
    pending_requests: Arc<PendingRequests>,
    request_counter: AtomicU64,
    capabilities: OnceLock<lsp::ServerCapabilities>,
    initialization: OnceCell<()>,
    pub(crate) file_operation_interest: OnceLock<FileOperationsInterest>,
    config: Option<Value>,
    root_path: std::path::PathBuf,
    root_uri: Option<lsp::Url>,
    workspace_folders: Mutex<Vec<lsp::WorkspaceFolder>>,
    initialize_notify: Arc<Notify>,
    /// Notified by the transport once `exit` has been flushed to the server's stdin.
    shutdown_flushed: Arc<Notify>,
    /// Maximum request duration in seconds, including time queued for writing.
    req_timeout: u64,
}

impl Client {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn id(&self) -> LanguageServerId {
        self.id
    }

    fn next_request_id(&self) -> jsonrpc::Id {
        let id = self.request_counter.fetch_add(1, Ordering::Relaxed);
        jsonrpc::Id::Num(id)
    }

    pub fn is_initialized(&self) -> bool {
        self.capabilities.get().is_some()
    }

    pub fn capabilities(&self) -> &lsp::ServerCapabilities {
        self.capabilities
            .get()
            .expect("language server not yet initialized!")
    }

    pub(crate) fn file_operations_interest(&self) -> &FileOperationsInterest {
        self.file_operation_interest
            .get_or_init(|| FileOperationsInterest::new(self.capabilities()))
    }

    pub fn offset_encoding(&self) -> OffsetEncoding {
        self.capabilities()
            .position_encoding
            .as_ref()
            .and_then(|encoding| match encoding.as_str() {
                "utf-8" => Some(OffsetEncoding::Utf8),
                "utf-16" => Some(OffsetEncoding::Utf16),
                "utf-32" => Some(OffsetEncoding::Utf32),
                encoding => {
                    log::error!("Server provided invalid position encoding {encoding}, defaulting to utf-16");
                    None
                },
            })
            .unwrap_or_default()
    }

    pub fn config(&self) -> Option<&Value> {
        self.config.as_ref()
    }

    /// Queue a request with the server's configured timeout and cancellation on drop.
    fn call<R: lsp::request::Request>(
        &self,
        params: R::Params,
    ) -> impl Future<Output = Result<R::Result>> + use<R>
    where
        R::Params: serde::Serialize,
    {
        self.call_with_ref::<R>(&params)
    }

    fn call_with_ref<R: lsp::request::Request>(
        &self,
        params: &R::Params,
    ) -> impl Future<Output = Result<R::Result>> + use<R>
    where
        R::Params: serde::Serialize,
    {
        self.call_with_timeout::<R>(params, self.req_timeout)
    }

    fn call_with_timeout<R: lsp::request::Request>(
        &self,
        params: &R::Params,
        timeout_secs: u64,
    ) -> impl Future<Output = Result<R::Result>> + use<R>
    where
        R::Params: serde::Serialize,
    {
        self.pending_requests.request::<R>(
            &self.server_tx,
            self.next_request_id(),
            params,
            timeout_secs,
        )
    }

    /// Send a RPC notification to the language server.
    pub fn notify<R: lsp::notification::Notification>(&self, params: R::Params)
    where
        R::Params: serde::Serialize,
    {
        let server_tx = self.server_tx.clone();

        let params = match serde_json::to_value(params) {
            Ok(params) => params,
            Err(err) => {
                log::error!(
                    "Failed to serialize params for notification '{}' for server '{}': {err}",
                    R::METHOD,
                    self.name,
                );
                return;
            }
        };

        let notification = jsonrpc::Notification {
            jsonrpc: Some(jsonrpc::Version::V2),
            method: R::METHOD.to_string(),
            params: params.into(),
        };

        if let Err(err) = server_tx.send(Payload::Notification(notification)) {
            log::error!(
                "Failed to send notification '{}' to server '{}': {err}",
                R::METHOD,
                self.name
            );
        }
    }

    /// Reply to a language server RPC call.
    pub fn reply(
        &self,
        id: jsonrpc::Id,
        result: core::result::Result<Value, jsonrpc::Error>,
    ) -> Result<()> {
        use jsonrpc::{Failure, Output, Success, Version};

        let server_tx = self.server_tx.clone();

        let output = match result {
            Ok(result) => Output::Success(Success {
                jsonrpc: Some(Version::V2),
                id,
                result,
            }),
            Err(error) => Output::Failure(Failure {
                jsonrpc: Some(Version::V2),
                id,
                error,
            }),
        };

        server_tx
            .send(Payload::Response(output))
            .map_err(|e| Error::Other(e.into()))?;

        Ok(())
    }
}
