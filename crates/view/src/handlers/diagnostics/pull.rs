//! Editor-owned pull diagnostics: scheduling, provider state, retries, and publication.

use std::{
    collections::{HashMap, HashSet},
    mem,
    time::Duration,
};

use editor_core::{diagnostic::DiagnosticProvider, syntax::config::LanguageServerFeature, Uri};
use event::{cancelable_future, register_hook, send_blocking, AsyncHook, TaskController};
use lsp_client::{lsp, LanguageServerId};
use tokio::{sync::mpsc::Sender, time::Instant};

use crate::{
    callbacks::EditorCallbackSender,
    events::{DocumentDidChange, DocumentDidOpen, LanguageServerExited, LanguageServerInitialized},
    handlers::lsp::DocumentRequest,
    DocumentId, Editor,
};

/// Cancellation and result IDs follow the document and are independent per server.
#[derive(Default)]
pub(crate) struct DocumentDiagnostics {
    result_ids: HashMap<LanguageServerId, String>,
    requests: HashMap<LanguageServerId, TaskController>,
}

/// Scheduling destinations shared by documents owned by this editor.
#[derive(Clone)]
pub struct PullDiagnosticsHandler {
    callbacks: EditorCallbackSender,
    documents: Sender<DocumentId>,
    inter_file: Sender<HashSet<LanguageServerId>>,
}

impl PullDiagnosticsHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let documents = DocumentDebounce {
            callbacks: callbacks.clone(),
            documents: HashSet::new(),
        }
        .spawn();
        let inter_file = InterFileDebounce {
            callbacks: callbacks.clone(),
            servers: HashSet::new(),
        }
        .spawn();
        Self {
            callbacks,
            documents,
            inter_file,
        }
    }
}

struct DocumentDebounce {
    callbacks: EditorCallbackSender,
    documents: HashSet<DocumentId>,
}

impl AsyncHook for DocumentDebounce {
    type Event = DocumentId;

    fn handle_event(&mut self, document: DocumentId, _: Option<Instant>) -> Option<Instant> {
        self.documents.insert(document);
        Some(Instant::now() + Duration::from_millis(250))
    }

    fn finish_debounce(&mut self) {
        let documents = mem::take(&mut self.documents);
        self.callbacks.send_blocking(move |editor| {
            for document in documents {
                request_document_diagnostics(editor, document);
            }
        });
    }
}

struct InterFileDebounce {
    callbacks: EditorCallbackSender,
    servers: HashSet<LanguageServerId>,
}

impl AsyncHook for InterFileDebounce {
    type Event = HashSet<LanguageServerId>;

    fn handle_event(&mut self, servers: Self::Event, _: Option<Instant>) -> Option<Instant> {
        self.servers.extend(servers);
        Some(Instant::now() + Duration::from_secs(1))
    }

    fn finish_debounce(&mut self) {
        let servers = mem::take(&mut self.servers);
        self.callbacks.send_blocking(move |editor| {
            let documents: Vec<_> = editor.documents.keys().copied().collect();
            for document in documents {
                request_document_diagnostics_for_language_servers(editor, document, &servers);
            }
        });
    }
}

fn request_document_diagnostics_for_language_servers(
    editor: &mut Editor,
    doc_id: DocumentId,
    servers: &HashSet<LanguageServerId>,
) {
    let callbacks = editor.handlers.pull_diagnostics.callbacks.clone();
    // Exit hooks run before the registry removes a server; queued refreshes may
    // also outlive a server. Only request providers still registered and attached.
    let mut servers: HashSet<_> = servers
        .iter()
        .copied()
        .filter(|&id| editor.language_server_by_id(id).is_some())
        .collect();
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    let Some(uri) = doc.uri() else {
        return;
    };
    let requests: Vec<_> = doc
        .language_servers_with_feature(LanguageServerFeature::PullDiagnostics)
        .filter(|server| servers.remove(&server.id()))
        .filter_map(|server| {
            let id = server.id();
            let future = server.text_document_diagnostic(
                doc.identifier(),
                doc.pull_diagnostics.result_ids.get(&id).cloned(),
            )?;
            let identifier =
                server
                    .capabilities()
                    .diagnostic_provider
                    .as_ref()
                    .and_then(|provider| match provider {
                        lsp::DiagnosticServerCapabilities::Options(options) => {
                            options.identifier.clone()
                        }
                        lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
                            options.diagnostic_options.identifier.clone()
                        }
                    });
            Some((
                id,
                DiagnosticProvider::Lsp {
                    server_id: id,
                    identifier,
                },
                future,
            ))
        })
        .collect();

    for (server_id, provider, future) in requests {
        let cancel = doc
            .pull_diagnostics
            .requests
            .entry(server_id)
            .or_default()
            .restart();
        let request = DocumentRequest::new(doc, cancel, vec![server_id]);
        let callbacks = callbacks.clone();
        let uri = uri.clone();
        tokio::spawn(async move {
            let Some(result) = cancelable_future(future, &request.cancel).await else {
                return;
            };
            match result {
                Ok(result) => {
                    callbacks
                        .send(move |editor| {
                            if request.is_current(editor) {
                                handle_pull_diagnostics_response(
                                    editor, result, provider, uri, doc_id,
                                );
                            }
                        })
                        .await;
                }
                Err(err) => {
                    let retrigger = if let lsp_client::Error::Rpc(error) = err {
                        error
                            .data
                            .and_then(|data| {
                                serde_json::from_value::<lsp::DiagnosticServerCancellationData>(
                                    data,
                                )
                                .ok()
                            })
                            .is_some_and(|data| data.retrigger_request)
                    } else {
                        log::error!("Pull diagnostic request failed: {err}");
                        false
                    };
                    if !retrigger
                        || cancelable_future(
                            tokio::time::sleep(Duration::from_millis(500)),
                            &request.cancel,
                        )
                        .await
                        .is_none()
                    {
                        return;
                    }
                    callbacks
                        .send(move |editor| {
                            if request.is_current(editor) {
                                request_document_diagnostics_for_language_servers(
                                    editor,
                                    doc_id,
                                    &HashSet::from([server_id]),
                                );
                            }
                        })
                        .await;
                }
            }
        });
    }
}

/// Refresh this provider for every open document attached to it.
pub fn request_all_document_diagnostics_for_language_server(
    editor: &mut Editor,
    server_id: LanguageServerId,
) {
    let documents: Vec<_> = editor
        .documents()
        .filter(|doc| doc.supports_language_server(server_id))
        .map(|doc| doc.id())
        .collect();
    let servers = HashSet::from([server_id]);
    for document in documents {
        request_document_diagnostics_for_language_servers(editor, document, &servers);
    }
}

/// Refresh all pull-diagnostic providers attached to this document.
pub fn request_document_diagnostics(editor: &mut Editor, doc_id: DocumentId) {
    let Some(doc) = editor.document(doc_id) else {
        return;
    };
    let servers = doc
        .language_servers_with_feature(LanguageServerFeature::PullDiagnostics)
        .map(|server| server.id())
        .collect();
    request_document_diagnostics_for_language_servers(editor, doc_id, &servers);
}

/// Register once; each event routes work through its owning editor or document.
pub fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut DocumentDidChange<'_>| {
            if event.ghost_transaction {
                return Ok(());
            }
            for controller in event.doc.pull_diagnostics.requests.values_mut() {
                controller.cancel();
            }
            if !event
                .doc
                .has_language_server_with_feature(LanguageServerFeature::PullDiagnostics)
            {
                return Ok(());
            }
            let Some(handler) = &event.doc.pull_diagnostics_handler else {
                return Ok(());
            };
            send_blocking(&handler.documents, event.doc.id());
            let servers: HashSet<_> = event
                .doc
                .language_servers_with_feature(LanguageServerFeature::PullDiagnostics)
                .filter(|server| {
                    server
                        .capabilities()
                        .diagnostic_provider
                        .as_ref()
                        .is_some_and(|provider| match provider {
                            lsp::DiagnosticServerCapabilities::Options(options) => {
                                options.inter_file_dependencies
                            }
                            lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
                                options.diagnostic_options.inter_file_dependencies
                            }
                        })
                })
                .map(|server| server.id())
                .collect();
            if !servers.is_empty() {
                send_blocking(&handler.inter_file, servers);
            }
            Ok(())
        });
        register_hook!(move |event: &mut DocumentDidOpen<'_>| {
            request_document_diagnostics(event.editor, event.doc);
            Ok(())
        });
        register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
            request_all_document_diagnostics_for_language_server(event.editor, event.server_id);
            Ok(())
        });
        register_hook!(move |event: &mut LanguageServerExited<'_>| {
            for doc in event.editor.documents_mut() {
                doc.pull_diagnostics.requests.remove(&event.server_id);
                doc.pull_diagnostics.result_ids.remove(&event.server_id);
            }
            Ok(())
        });
    });
}
fn handle_pull_diagnostics_response(
    editor: &mut Editor,
    result: lsp::DocumentDiagnosticReportResult,
    provider: DiagnosticProvider,
    uri: Uri,
    document_id: DocumentId,
) {
    match result {
        lsp::DocumentDiagnosticReportResult::Report(report) => {
            let result_id = match report {
                lsp::DocumentDiagnosticReport::Full(report) => {
                    editor.handle_lsp_diagnostics(
                        &provider,
                        uri,
                        None,
                        report.full_document_diagnostic_report.items,
                    );

                    report.full_document_diagnostic_report.result_id
                }
                lsp::DocumentDiagnosticReport::Unchanged(report) => {
                    Some(report.unchanged_document_diagnostic_report.result_id)
                }
            };

            if let Some(doc) = editor.document_mut(document_id) {
                let server_id = provider
                    .language_server_id()
                    .expect("pull diagnostics always originate from an LSP");
                match result_id {
                    Some(result_id) => {
                        doc.pull_diagnostics.result_ids.insert(server_id, result_id);
                    }
                    None => {
                        doc.pull_diagnostics.result_ids.remove(&server_id);
                    }
                }
            };
        }
        lsp::DocumentDiagnosticReportResult::Partial(_) => {}
    };
}
