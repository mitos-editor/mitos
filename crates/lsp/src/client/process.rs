//! Language-server process startup, initialization, and shutdown.

use super::{workspace::workspace_for_uri, Client};
use crate::{
    jsonrpc,
    lsp::{self, CodeActionCapabilityResolveSupport, PositionEncodingKind},
    transport::Transport,
    Call, Error, LanguageServerId, Result,
};
use log::info;
use parking_lot::Mutex;
use serde_json::Value;
use std::{
    ffi::OsStr,
    path::PathBuf,
    process::Stdio,
    sync::{atomic::AtomicU64, OnceLock},
};
use tokio::{
    io::{BufReader, BufWriter},
    process::Command,
    sync::{mpsc::UnboundedReceiver, OnceCell},
};

impl Client {
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    /// Spawns a language server and starts its JSON-RPC transport.
    ///
    /// The caller must subsequently call [`Self::initialize`]. The returned
    /// receiver carries server-originated calls. Until initialization succeeds,
    /// the transport queues requests and discards ordinary notifications.
    pub fn start(
        cmd: &str,
        args: &[String],
        config: Option<Value>,
        server_environment: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
        root_path: PathBuf,
        root_uri: Option<lsp::Url>,
        id: LanguageServerId,
        name: String,
        req_timeout: u64,
    ) -> Result<(Self, UnboundedReceiver<(LanguageServerId, Call)>)> {
        info!("Starting lsp {name:?} in root {root_path:?}");
        // Resolve path to the binary
        let cmd = stdx::env::which(cmd)?;

        let process = Command::new(cmd)
            .envs(server_environment)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&root_path)
            // make sure the process is reaped on drop
            .kill_on_drop(true)
            .spawn();

        let mut process = process?;

        // TODO: do we need bufreader/writer here? or do we use async wrappers on unblock?
        let writer = BufWriter::new(process.stdin.take().expect("Failed to open stdin"));
        let reader = BufReader::new(process.stdout.take().expect("Failed to open stdout"));
        let stderr = BufReader::new(process.stderr.take().expect("Failed to open stderr"));

        let connection = Transport::start(reader, writer, stderr, id, name.clone());

        let workspace_folders = root_uri
            .clone()
            .map(|root| vec![workspace_for_uri(root)])
            .unwrap_or_default();

        let client = Self {
            id,
            name,
            _process: process,
            server_tx: connection.sender,
            pending_requests: connection.requests,
            request_counter: AtomicU64::new(0),
            capabilities: OnceLock::new(),
            initialization: OnceCell::new(),
            file_operation_interest: OnceLock::new(),
            config,
            req_timeout,
            root_path,
            root_uri,
            workspace_folders: Mutex::new(workspace_folders),
            initialize_notify: connection.initialized.clone(),
            shutdown_flushed: connection.shutdown_flushed,
        };

        Ok((client, connection.receiver))
    }

    /// Negotiate capabilities once and release requests queued during startup.
    pub async fn initialize(
        &self,
        enable_snippets: bool,
        client_info: lsp::ClientInfo,
    ) -> Result<()> {
        self.initialization
            .get_or_try_init(|| async {
                let response = self
                    .initialize_request(enable_snippets, client_info)
                    .await?;
                self.capabilities
                    .set(response.capabilities)
                    .expect("capabilities are installed by one initialization");
                self.notify::<lsp::notification::Initialized>(lsp::InitializedParams {});
                self.initialize_notify.notify_one();
                Ok::<_, Error>(())
            })
            .await?;
        Ok(())
    }

    async fn initialize_request(
        &self,
        enable_snippets: bool,
        client_info: lsp::ClientInfo,
    ) -> Result<lsp::InitializeResult> {
        if let Some(config) = &self.config {
            log::info!("Using custom LSP config: {}", config);
        }

        #[allow(deprecated)]
        let params = lsp::InitializeParams {
            process_id: Some(std::process::id()),
            workspace_folders: Some(self.workspace_folders.lock().clone()),
            // root_path is obsolete, but some clients like pyright still use it so we specify both.
            // clients will prefer _uri if possible
            root_path: self.root_path.to_str().map(|path| path.to_owned()),
            root_uri: self.root_uri.clone(),
            initialization_options: self.config.clone(),
            capabilities: lsp::ClientCapabilities {
                workspace: Some(lsp::WorkspaceClientCapabilities {
                    configuration: Some(true),
                    did_change_configuration: Some(lsp::DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    workspace_folders: Some(true),
                    apply_edit: Some(true),
                    symbol: Some(lsp::WorkspaceSymbolClientCapabilities {
                        dynamic_registration: Some(false),
                        symbol_kind: Some(lsp::SymbolKindCapability {
                            value_set: Some(lsp::SymbolKind::all()),
                        }),
                        ..Default::default()
                    }),
                    execute_command: Some(lsp::DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    inlay_hint: Some(lsp::InlayHintWorkspaceClientCapabilities {
                        refresh_support: Some(false),
                    }),
                    inline_value: Some(lsp::InlineValueWorkspaceClientCapabilities {
                        refresh_support: Some(true),
                    }),
                    workspace_edit: Some(lsp::WorkspaceEditClientCapabilities {
                        document_changes: Some(true),
                        resource_operations: Some(vec![
                            lsp::ResourceOperationKind::Create,
                            lsp::ResourceOperationKind::Rename,
                            lsp::ResourceOperationKind::Delete,
                        ]),
                        failure_handling: Some(lsp::FailureHandlingKind::Abort),
                        normalizes_line_endings: Some(false),
                        change_annotation_support: None,
                    }),
                    did_change_watched_files: Some(lsp::DidChangeWatchedFilesClientCapabilities {
                        dynamic_registration: Some(true),
                        relative_pattern_support: Some(true),
                    }),
                    file_operations: Some(lsp::WorkspaceFileOperationsClientCapabilities {
                        will_create: Some(true),
                        did_create: Some(true),
                        will_rename: Some(true),
                        did_rename: Some(true),
                        will_delete: Some(true),
                        did_delete: Some(true),
                        ..Default::default()
                    }),
                    diagnostic: Some(lsp::DiagnosticWorkspaceClientCapabilities {
                        refresh_support: Some(true),
                    }),
                    code_lens: Some(lsp::CodeLensWorkspaceClientCapabilities {
                        refresh_support: Some(true),
                    }),
                    ..Default::default()
                }),
                text_document: Some(lsp::TextDocumentClientCapabilities {
                    completion: Some(lsp::CompletionClientCapabilities {
                        completion_item: Some(lsp::CompletionItemCapability {
                            snippet_support: Some(enable_snippets),
                            resolve_support: Some(lsp::CompletionItemCapabilityResolveSupport {
                                properties: vec![
                                    String::from("documentation"),
                                    String::from("detail"),
                                    String::from("additionalTextEdits"),
                                ],
                            }),
                            insert_replace_support: Some(true),
                            deprecated_support: Some(true),
                            tag_support: Some(lsp::TagSupport {
                                value_set: vec![lsp::CompletionItemTag::DEPRECATED],
                            }),
                            ..Default::default()
                        }),
                        completion_item_kind: Some(lsp::CompletionItemKindCapability {
                            ..Default::default()
                        }),
                        context_support: None, // additional context information Some(true)
                        ..Default::default()
                    }),
                    hover: Some(lsp::HoverClientCapabilities {
                        // if not specified, rust-analyzer returns plaintext marked as markdown but
                        // badly formatted.
                        content_format: Some(vec![lsp::MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    signature_help: Some(lsp::SignatureHelpClientCapabilities {
                        signature_information: Some(lsp::SignatureInformationSettings {
                            documentation_format: Some(vec![lsp::MarkupKind::Markdown]),
                            parameter_information: Some(lsp::ParameterInformationSettings {
                                label_offset_support: Some(true),
                            }),
                            active_parameter_support: Some(true),
                        }),
                        ..Default::default()
                    }),
                    rename: Some(lsp::RenameClientCapabilities {
                        dynamic_registration: Some(false),
                        prepare_support: Some(true),
                        prepare_support_default_behavior: None,
                        honors_change_annotations: Some(false),
                    }),
                    formatting: Some(lsp::DocumentFormattingClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    code_action: Some(lsp::CodeActionClientCapabilities {
                        code_action_literal_support: Some(lsp::CodeActionLiteralSupport {
                            code_action_kind: lsp::CodeActionKindLiteralSupport {
                                value_set: [
                                    lsp::CodeActionKind::EMPTY,
                                    lsp::CodeActionKind::QUICKFIX,
                                    lsp::CodeActionKind::REFACTOR,
                                    lsp::CodeActionKind::REFACTOR_EXTRACT,
                                    lsp::CodeActionKind::REFACTOR_INLINE,
                                    lsp::CodeActionKind::REFACTOR_REWRITE,
                                    lsp::CodeActionKind::SOURCE,
                                    lsp::CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                                    lsp::CodeActionKind::SOURCE_FIX_ALL,
                                ]
                                .iter()
                                .map(|kind| kind.as_str().to_string())
                                .collect(),
                            },
                        }),
                        is_preferred_support: Some(true),
                        disabled_support: Some(true),
                        data_support: Some(true),
                        resolve_support: Some(CodeActionCapabilityResolveSupport {
                            properties: vec!["edit".to_owned(), "command".to_owned()],
                        }),
                        ..Default::default()
                    }),
                    diagnostic: Some(lsp::DiagnosticClientCapabilities {
                        dynamic_registration: Some(false),
                        related_document_support: Some(true),
                    }),
                    publish_diagnostics: Some(lsp::PublishDiagnosticsClientCapabilities {
                        version_support: Some(true),
                        tag_support: Some(lsp::TagSupport {
                            value_set: vec![
                                lsp::DiagnosticTag::UNNECESSARY,
                                lsp::DiagnosticTag::DEPRECATED,
                            ],
                        }),
                        ..Default::default()
                    }),
                    inline_completion: Some(lsp::InlineCompletionClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    inline_value: Some(lsp::InlineValueClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    code_lens: Some(lsp::CodeLensClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    inlay_hint: Some(lsp::InlayHintClientCapabilities {
                        dynamic_registration: Some(false),
                        resolve_support: None,
                    }),
                    document_link: Some(lsp::DocumentLinkClientCapabilities {
                        dynamic_registration: Some(false),
                        tooltip_support: Some(false),
                    }),
                    call_hierarchy: Some(lsp::DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    document_symbol: Some(lsp::DocumentSymbolClientCapabilities {
                        dynamic_registration: Some(false),
                        symbol_kind: Some(lsp::SymbolKindCapability {
                            value_set: Some(lsp::SymbolKind::all()),
                        }),
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                window: Some(lsp::WindowClientCapabilities {
                    show_message: Some(lsp::ShowMessageRequestClientCapabilities {
                        message_action_item: Some(lsp::MessageActionItemCapabilities {
                            additional_properties_support: Some(true),
                        }),
                    }),
                    work_done_progress: Some(true),
                    show_document: Some(lsp::ShowDocumentClientCapabilities { support: true }),
                }),
                general: Some(lsp::GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        PositionEncodingKind::UTF8,
                        PositionEncodingKind::UTF32,
                        PositionEncodingKind::UTF16,
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            trace: None,
            client_info: Some(client_info),
            locale: None, // TODO
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
        };

        self.call::<lsp::request::Initialize>(params).await
    }

    /// Requests graceful shutdown without sending `exit`.
    pub async fn shutdown(&self) -> Result<()> {
        self.call::<lsp::request::Shutdown>(()).await
    }

    /// Queues the terminal LSP `exit` notification.
    pub fn exit(&self) {
        self.notify::<lsp::notification::Exit>(())
    }

    /// Requests graceful shutdown and then sends `exit`.
    ///
    /// Returns early without sending `exit` when the server rejects or fails
    /// the shutdown request.
    pub async fn shutdown_and_exit(&self) -> Result<()> {
        self.shutdown().await?;
        self.exit();
        Ok(())
    }

    /// Sends the LSP shutdown request followed immediately by the exit
    /// notification, without waiting for the shutdown response (fire-and-forget).
    /// The server receives both in order and exits on its own; any late shutdown
    /// response is discarded. Quitting therefore never blocks on a slow server
    /// (e.g. gopls flushing ~1k log messages before it would answer `shutdown`).
    ///
    /// The child process is not waited on here: it is killed by `kill_on_drop`
    /// when this `Client` is dropped. `close_language_servers` first gives a short
    /// grace window so the `exit` reaches stdin and well-behaved servers can begin
    /// exiting before the kill.
    pub fn force_shutdown(&self) {
        let request = jsonrpc::MethodCall {
            jsonrpc: Some(jsonrpc::Version::V2),
            id: self.next_request_id(),
            method: <lsp::request::Shutdown as lsp::request::Request>::METHOD.to_string(),
            params: jsonrpc::Params::None,
        };
        let _ = self
            .pending_requests
            .send_fire_and_forget(&self.server_tx, request);
        self.exit();
    }

    /// Resolves once the `exit` notification has been written to the server's stdin
    /// (i.e. mitos's *outbound* write completed — independent of how slow the server
    /// is, since it doesn't wait for any server response).
    pub async fn wait_shutdown_flushed(&self) {
        self.shutdown_flushed.notified().await;
    }
}
