//! Language Server Protocol process, transport, and editor conversions.
//!
//! [`Client`] owns one language-server child process. Requests are paired with
//! responses inside the transport, while server-originated requests and
//! notifications are exposed as [`Call`] values for the editor event loop.
//! The registry in this crate chooses and reuses clients by language-server
//! configuration and workspace root.
//!
//! LSP positions are not editor positions. Mitos stores rope positions as Rust
//! character indices, while a server may use UTF-8, UTF-16, or UTF-32 code
//! units. All crossings should go through [`util`] with the client's negotiated
//! [`OffsetEncoding`]; hand-written line/column arithmetic is almost always a
//! bug for non-ASCII text.
//!
//! Protocol wire types are available under [`lsp`] and intentionally remain in
//! the separate `lsp-types` crate.

mod client;
pub mod file_event;
mod file_operations;
pub mod jsonrpc;
mod registry;
mod transport;
pub use registry::{find_lsp_workspace, has_required_root, language_server_root, Registry};

pub use client::Client;
pub use futures_executor::block_on;
pub use jsonrpc::Call;
pub use lsp::{Position, Url};
pub use lsp_types as lsp;
use std::collections::HashMap;

use thiserror::Error;

/// Result type used by LSP client and transport operations.
pub type Result<T, E = Error> = core::result::Result<T, E>;
/// User-configured name of a language-server definition.
pub type LanguageServerName = String;
pub use editor_core::diagnostic::LanguageServerId;

#[derive(Error, Debug)]
/// Errors raised while starting a server, exchanging JSON-RPC messages, or
/// decoding protocol payloads.
pub enum Error {
    #[error("protocol error: {0}")]
    Rpc(#[from] jsonrpc::Error),
    #[error("failed to parse: {0}")]
    Parse(Box<dyn std::error::Error + Send + Sync>),
    #[error("IO Error: {0}")]
    IO(#[from] std::io::Error),
    #[error("request {0} timed out")]
    Timeout(jsonrpc::Id),
    #[error("server closed the stream")]
    StreamClosed,
    #[error("Unhandled")]
    Unhandled,
    #[error(transparent)]
    ExecutableNotFound(#[from] stdx::env::ExecutableNotFoundError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Parse(Box::new(value))
    }
}

impl From<sonic_rs::Error> for Error {
    fn from(value: sonic_rs::Error) -> Self {
        Self::Parse(Box::new(value))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Unit used by the server for the `character` component of LSP positions.
///
/// UTF-16 is the protocol default when position encoding negotiation is not
/// supported.
pub enum OffsetEncoding {
    /// UTF-8 code units aka bytes
    Utf8,
    /// UTF-32 code units aka chars
    Utf32,
    /// UTF-16 code units
    #[default]
    Utf16,
}

pub mod util;
#[derive(Debug, PartialEq, Clone)]
pub enum MethodCall {
    WorkDoneProgressCreate(lsp::WorkDoneProgressCreateParams),
    ApplyWorkspaceEdit(lsp::ApplyWorkspaceEditParams),
    WorkspaceFolders,
    WorkspaceConfiguration(lsp::ConfigurationParams),
    RegisterCapability(lsp::RegistrationParams),
    UnregisterCapability(lsp::UnregistrationParams),
    ShowDocument(lsp::ShowDocumentParams),
    WorkspaceDiagnosticRefresh,
    CodeLensRefresh,
    ShowMessageRequest(lsp::ShowMessageRequestParams),
}

impl MethodCall {
    pub fn parse(method: &str, params: jsonrpc::Params) -> Result<MethodCall> {
        use lsp::request::Request;
        let request = match method {
            lsp::request::WorkDoneProgressCreate::METHOD => {
                let params: lsp::WorkDoneProgressCreateParams = params.parse()?;
                Self::WorkDoneProgressCreate(params)
            }
            lsp::request::ApplyWorkspaceEdit::METHOD => {
                let params: lsp::ApplyWorkspaceEditParams = params.parse()?;
                Self::ApplyWorkspaceEdit(params)
            }
            lsp::request::WorkspaceFoldersRequest::METHOD => Self::WorkspaceFolders,
            lsp::request::WorkspaceConfiguration::METHOD => {
                let params: lsp::ConfigurationParams = params.parse()?;
                Self::WorkspaceConfiguration(params)
            }
            lsp::request::RegisterCapability::METHOD => {
                let params: lsp::RegistrationParams = params.parse()?;
                Self::RegisterCapability(params)
            }
            lsp::request::UnregisterCapability::METHOD => {
                let params: lsp::UnregistrationParams = params.parse()?;
                Self::UnregisterCapability(params)
            }
            lsp::request::ShowDocument::METHOD => {
                let params: lsp::ShowDocumentParams = params.parse()?;
                Self::ShowDocument(params)
            }
            lsp::request::WorkspaceDiagnosticRefresh::METHOD => Self::WorkspaceDiagnosticRefresh,
            lsp::request::CodeLensRefresh::METHOD => Self::CodeLensRefresh,
            lsp::request::ShowMessageRequest::METHOD => {
                let params: lsp::ShowMessageRequestParams = params.parse()?;
                Self::ShowMessageRequest(params)
            }
            _ => {
                return Err(Error::Unhandled);
            }
        };
        Ok(request)
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Notification {
    // we inject this notification to signal the LSP is ready
    Initialized,
    // and this notification to signal that the LSP exited
    Exit,
    PublishDiagnostics(lsp::PublishDiagnosticsParams),
    ShowMessage(lsp::ShowMessageParams),
    LogMessage(lsp::LogMessageParams),
    ProgressMessage(lsp::ProgressParams),
}

impl Notification {
    pub fn parse(method: &str, params: jsonrpc::Params) -> Result<Notification> {
        use lsp::notification::Notification as _;

        let notification = match method {
            lsp::notification::Initialized::METHOD => Self::Initialized,
            lsp::notification::Exit::METHOD => Self::Exit,
            lsp::notification::PublishDiagnostics::METHOD => {
                let params: lsp::PublishDiagnosticsParams = params.parse()?;
                Self::PublishDiagnostics(params)
            }

            lsp::notification::ShowMessage::METHOD => {
                let params: lsp::ShowMessageParams = params.parse()?;
                Self::ShowMessage(params)
            }
            lsp::notification::LogMessage::METHOD => {
                let params: lsp::LogMessageParams = params.parse()?;
                Self::LogMessage(params)
            }
            lsp::notification::Progress::METHOD => {
                let params: lsp::ProgressParams = params.parse()?;
                Self::ProgressMessage(params)
            }
            _ => {
                return Err(Error::Unhandled);
            }
        };

        Ok(notification)
    }
}

#[derive(Debug)]
pub enum ProgressStatus {
    Created,
    Started {
        title: String,
        progress: lsp::WorkDoneProgress,
    },
}

impl ProgressStatus {
    pub fn progress(&self) -> Option<&lsp::WorkDoneProgress> {
        match &self {
            ProgressStatus::Created => None,
            ProgressStatus::Started { title: _, progress } => Some(progress),
        }
    }
}

#[derive(Default, Debug)]
/// Acts as a container for progress reported by language servers. Each server
/// has a unique id assigned at creation through [`Registry`]. This id is then used
/// to store the progress in this map.
pub struct LspProgressMap(HashMap<LanguageServerId, HashMap<lsp::ProgressToken, ProgressStatus>>);

impl LspProgressMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a map of all tokens corresponding to the language server with `id`.
    pub fn progress_map(
        &self,
        id: LanguageServerId,
    ) -> Option<&HashMap<lsp::ProgressToken, ProgressStatus>> {
        self.0.get(&id)
    }

    pub fn is_progressing(&self, id: LanguageServerId) -> bool {
        self.0.get(&id).map(|it| !it.is_empty()).unwrap_or_default()
    }

    /// Returns last progress status for a given server with `id` and `token`.
    pub fn progress(
        &self,
        id: LanguageServerId,
        token: &lsp::ProgressToken,
    ) -> Option<&ProgressStatus> {
        self.0.get(&id).and_then(|values| values.get(token))
    }

    pub fn title(&self, id: LanguageServerId, token: &lsp::ProgressToken) -> Option<&String> {
        self.progress(id, token).and_then(|p| match p {
            ProgressStatus::Created => None,
            ProgressStatus::Started { title, .. } => Some(title),
        })
    }

    /// Checks if progress `token` for server with `id` is created.
    pub fn is_created(&mut self, id: LanguageServerId, token: &lsp::ProgressToken) -> bool {
        self.0
            .get(&id)
            .map(|values| values.get(token).is_some())
            .unwrap_or_default()
    }

    pub fn create(&mut self, id: LanguageServerId, token: lsp::ProgressToken) {
        self.0
            .entry(id)
            .or_default()
            .insert(token, ProgressStatus::Created);
    }

    /// Ends the progress by removing the `token` from server with `id`, if removed returns the value.
    pub fn end_progress(
        &mut self,
        id: LanguageServerId,
        token: &lsp::ProgressToken,
    ) -> Option<ProgressStatus> {
        self.0.get_mut(&id).and_then(|vals| vals.remove(token))
    }

    /// Updates the progress of `token` for server with `id` to begin state `status`
    pub fn begin(
        &mut self,
        id: LanguageServerId,
        token: lsp::ProgressToken,
        status: lsp::WorkDoneProgressBegin,
    ) {
        self.0.entry(id).or_default().insert(
            token,
            ProgressStatus::Started {
                title: status.title.clone(),
                progress: lsp::WorkDoneProgress::Begin(status),
            },
        );
    }

    /// Updates the progress of `token` for server with `id` to report state `status`.
    pub fn update(
        &mut self,
        id: LanguageServerId,
        token: lsp::ProgressToken,
        status: lsp::WorkDoneProgressReport,
    ) {
        self.0
            .entry(id)
            .or_default()
            .entry(token)
            .and_modify(|e| match e {
                ProgressStatus::Created => (),
                ProgressStatus::Started { progress, .. } => {
                    *progress = lsp::WorkDoneProgress::Report(status)
                }
            });
    }
}
