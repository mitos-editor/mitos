//! Editor-owned pull diagnostics: scheduling, provider state, retries, and publication.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    mem,
    time::Duration,
};

use ::diagnostics::DiagnosticProvider;
use editor_core::Uri;
use event::{cancelable_future, register_hook, send_blocking, AsyncHook, TaskController};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId, UriExt as _};
use tokio::{sync::mpsc::Sender, time::Instant};

use crate::{
    callbacks::EditorCallbackSender,
    events::{
        DocumentDidChange, DocumentDidOpen, DocumentLanguageDidChange, LanguageServerExited,
        LanguageServerInitialized,
    },
    features::{debounce::debounce_documents, request::DocumentRequest},
    DocumentId, Editor,
};

/// Cancellation follows the document and is independent per server.
#[derive(Default)]
pub(crate) struct DocumentDiagnostics {
    requests: HashMap<LanguageServerId, TaskController>,
    generation: u64,
}

impl DocumentDiagnostics {
    pub(crate) fn clear(&mut self) {
        self.requests.clear();
        self.generation += 1;
    }
}

/// Result IDs and request ordering survive while a reported file is unopened.
#[derive(Default)]
pub(crate) struct PullDiagnosticResults {
    next_request: u64,
    reports: BTreeMap<Uri, HashMap<DiagnosticProvider, DiagnosticReport>>,
}

#[derive(Default)]
struct DiagnosticReport {
    request: u64,
    result_id: Option<String>,
    has_report: bool,
}

impl PullDiagnosticResults {
    fn begin(&mut self, uri: &Uri, provider: &DiagnosticProvider) -> (u64, Option<String>) {
        self.next_request += 1;
        let report = self
            .reports
            .entry(uri.clone())
            .or_default()
            .entry(provider.clone())
            .or_default();
        report.request = self.next_request;
        (report.request, report.result_id.clone())
    }

    fn invalidate(&mut self, uri: &Uri) {
        self.next_request += 1;
        if let Some(reports) = self.reports.get_mut(uri) {
            for report in reports.values_mut() {
                *report = DiagnosticReport {
                    request: self.next_request,
                    ..Default::default()
                };
            }
        }
    }
}

struct RelatedDocument {
    request: DocumentRequest,
    generation: u64,
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
        let documents = debounce_documents(callbacks.clone(), request_document_diagnostics);
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
    let Some(doc) = editor.documents.get(&doc_id) else {
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
            let identifier =
                server
                    .capabilities()
                    .diagnostic_provider
                    .as_ref()
                    .and_then(|provider| match provider {
                        lsp::DiagnosticServerCapabilities::Options(options) => {
                            options.identifier.as_deref().map(Into::into)
                        }
                        lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => options
                            .diagnostic_options
                            .identifier
                            .as_deref()
                            .map(Into::into),
                    });
            let provider = DiagnosticProvider::Lsp {
                server_id: id,
                identifier,
            };
            let (request_id, previous_result_id) = editor.pull_diagnostics.begin(&uri, &provider);
            let future = server.text_document_diagnostic(doc.identifier(), previous_result_id)?;
            Some((id, provider, request_id, future))
        })
        .collect();

    for (server_id, provider, request_id, future) in requests {
        let doc = editor.documents.get_mut(&doc_id).unwrap();
        let cancel = doc
            .pull_diagnostics
            .requests
            .entry(server_id)
            .or_default()
            .restart();
        let request = DocumentRequest::new(doc, cancel, vec![server_id]);
        // Related reports have no document versions. Retain the target sessions
        // so a queued response cannot publish across edits, renames, or reopening.
        let related_documents = editor
            .documents()
            .filter_map(|doc| {
                Some((
                    doc.uri()?,
                    RelatedDocument {
                        request: DocumentRequest::new(doc, request.cancel.clone(), vec![server_id]),
                        generation: doc.pull_diagnostics.generation,
                    },
                ))
            })
            .collect();
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
                                    editor,
                                    result,
                                    provider,
                                    uri,
                                    request_id,
                                    related_documents,
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

/// Each event routes work through its owning editor or document.
pub(crate) fn register_hooks(events: &event::EventHub, hooks: crate::handlers::DocumentHooks) {
    register_hook!(events => move |event: &mut DocumentLanguageDidChange<'_>| {
        if let Some(uri) = event.editor.document(event.doc).and_then(|doc| doc.uri()) {
            event.editor.pull_diagnostics.invalidate(&uri);
        }
        request_document_diagnostics(event.editor, event.doc);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentDidChange<'_>| {
        for controller in event.doc.pull_diagnostics.requests.values_mut() {
            controller.cancel();
        }
        if !event
            .doc
            .has_language_server_with_feature(LanguageServerFeature::PullDiagnostics)
        {
            return Ok(());
        }
        if event.doc.is_content_readonly() {
            return Ok(());
        }
        let Some(services) = hooks.upgrade() else {
            return Ok(());
        };
        let Some(services) = services.get() else {
            return Ok(());
        };
        let handler = &services.pull_diagnostics;
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
    register_hook!(events => move |event: &mut DocumentDidOpen<'_>| {
        request_document_diagnostics(event.editor, event.doc);
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerInitialized<'_>| {
        request_all_document_diagnostics_for_language_server(event.editor, event.server_id);
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            doc.pull_diagnostics.requests.remove(&event.server_id);
        }
        event.editor.pull_diagnostics.reports.retain(|_, reports| {
            reports.retain(|provider, _| provider.language_server_id() != Some(event.server_id));
            !reports.is_empty()
        });
        Ok(())
    });
}

fn handle_pull_diagnostics_response(
    editor: &mut Editor,
    result: lsp::DocumentDiagnosticReportResult,
    provider: DiagnosticProvider,
    uri: Uri,
    request_id: u64,
    related_documents: BTreeMap<Uri, RelatedDocument>,
) {
    let (primary, related) = match result {
        lsp::DocumentDiagnosticReportResult::Report(lsp::DocumentDiagnosticReport::Full(
            report,
        )) => (
            Some(report.full_document_diagnostic_report.into()),
            report.related_documents,
        ),
        lsp::DocumentDiagnosticReportResult::Report(lsp::DocumentDiagnosticReport::Unchanged(
            report,
        )) => (
            Some(report.unchanged_document_diagnostic_report.into()),
            report.related_documents,
        ),
        lsp::DocumentDiagnosticReportResult::Partial(report) => (None, report.related_documents),
    };
    if let Some(report) = primary {
        apply_report(editor, &provider, uri.clone(), request_id, report);
    }

    for (related_uri, report) in related.into_iter().flatten() {
        let related_uri = match related_uri.to_editor_uri() {
            Ok(uri) => uri,
            Err(error) => {
                log::warn!("Ignoring related diagnostic report: {error}");
                continue;
            }
        };
        if related_uri == uri {
            continue;
        }
        // Related reports have no previousResultId parameter. Only reuse an
        // unchanged result if its full report was actually retained.
        if let lsp::DocumentDiagnosticReportKind::Unchanged(report) = &report
            && !editor
                .pull_diagnostics
                .reports
                .get(&related_uri)
                .and_then(|reports| reports.get(&provider))
                .is_some_and(|cached| cached.result_id.as_ref() == Some(&report.result_id))
        {
            continue;
        }
        let current = match related_documents.get(&related_uri) {
            Some(target) => target
                .request
                .get(editor)
                .is_some_and(|doc| doc.pull_diagnostics.generation == target.generation),
            None => !editor
                .documents()
                .any(|doc| doc.uri().as_ref() == Some(&related_uri)),
        };
        if current {
            apply_report(editor, &provider, related_uri, request_id, report);
        }
    }
}

fn apply_report(
    editor: &mut Editor,
    provider: &DiagnosticProvider,
    uri: Uri,
    request_id: u64,
    report: lsp::DocumentDiagnosticReportKind,
) {
    let cached = editor
        .pull_diagnostics
        .reports
        .entry(uri.clone())
        .or_default()
        .entry(provider.clone())
        .or_default();
    // Direct pulls reserve their place when issued. A late related report must
    // not replace a newer pull, including one whose response is still pending.
    if cached.request > request_id {
        return;
    }
    match report {
        lsp::DocumentDiagnosticReportKind::Full(report) => {
            cached.request = request_id;
            cached.result_id = report.result_id;
            cached.has_report = true;
            editor.handle_lsp_diagnostics(provider, uri, None, report.items);
        }
        lsp::DocumentDiagnosticReportKind::Unchanged(report) if cached.has_report => {
            cached.request = request_id;
            cached.result_id = Some(report.result_id);
        }
        // A previous full report may have been discarded as stale. Without its
        // diagnostics, retaining this ID could make subsequent pulls skip them.
        lsp::DocumentDiagnosticReportKind::Unchanged(_) => {}
    }
}
