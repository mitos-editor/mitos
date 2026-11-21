//! Workspace pulls share document/provider result IDs without replacing direct pulls.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
    time::Duration,
};

use diagnostics::DiagnosticProvider;
use editor_core::Uri;
use event::{cancelable_future, TaskHandle};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, Client, LanguageServerId, UriExt as _};

use super::apply_report;
use crate::{DocumentId, Editor};

struct OpenDocument {
    id: DocumentId,
    version: i32,
    generation: u64,
}

struct WorkspacePull {
    server: Weak<Client>,
    provider: DiagnosticProvider,
    request_id: u64,
    cancel: TaskHandle,
    documents: BTreeMap<Uri, OpenDocument>,
    previous_results: BTreeMap<Uri, String>,
    // An unchanged batch can refer to the latest full batch in this stream,
    // including one rejected as stale. Never associate its ID with older data.
    discarded_full: parking_lot::Mutex<BTreeSet<Uri>>,
}

fn has_documents(editor: &Editor, server_id: LanguageServerId) -> bool {
    editor.config().lsp.enable
        && editor.documents().any(|doc| {
            doc.language_servers_with_configured_feature(LanguageServerFeature::PullDiagnostics)
                .any(|server| server.id() == server_id)
        })
}

impl WorkspacePull {
    fn is_current(&self, editor: &Editor) -> bool {
        !self.cancel.is_canceled()
            && self.server.upgrade().is_some_and(|server| {
                has_documents(editor, server.id())
                    && editor
                        .language_servers
                        .get_by_id(server.id())
                        .is_some_and(|current| Arc::ptr_eq(current, &server))
            })
    }

    fn apply(&self, editor: &mut Editor, report: lsp::WorkspaceDiagnosticReport) {
        if !self.is_current(editor) {
            return;
        }
        for item in report.items {
            let (uri, version, report) = match item {
                lsp::WorkspaceDocumentDiagnosticReport::Full(report) => (
                    report.uri,
                    report.version,
                    report.full_document_diagnostic_report.into(),
                ),
                lsp::WorkspaceDocumentDiagnosticReport::Unchanged(report) => (
                    report.uri,
                    report.version,
                    report.unchanged_document_diagnostic_report.into(),
                ),
            };
            let Ok(uri) = uri.to_editor_uri() else {
                continue;
            };
            let full = matches!(&report, lsp::DocumentDiagnosticReportKind::Full(_));
            if full {
                self.discarded_full.lock().insert(uri.clone());
            }
            let open = editor
                .documents()
                .find(|doc| doc.uri().as_ref() == Some(&uri));
            match (self.documents.get(&uri), open) {
                (Some(target), Some(doc)) => {
                    // Only this provider's direct pulls have precedence. A
                    // workspace can also report a file open in another language
                    // or with this server's direct diagnostic feature disabled.
                    if doc
                        .language_servers_with_feature(LanguageServerFeature::PullDiagnostics)
                        .any(|server| Some(server.id()) == self.provider.language_server_id())
                    {
                        continue;
                    }
                    if doc.id() != target.id
                        || doc.pull_diagnostics.generation != target.generation
                        || !version.map_or(doc.version() == target.version, |version| {
                            version == doc.version()
                        })
                    {
                        continue;
                    }
                }
                // A report with a matching version may still belong to a
                // previous document session. Never cross opening or closure.
                (Some(_), None) | (None, Some(_)) => continue,
                (None, None) => {}
            }
            if let lsp::DocumentDiagnosticReportKind::Unchanged(_) = &report
                && (self.discarded_full.lock().contains(&uri)
                    || !editor
                        .pull_diagnostics
                        .reports
                        .get(&uri)
                        .and_then(|reports| reports.get(&self.provider))
                        .is_some_and(|cached| {
                            cached.has_report
                                && (cached.request == self.request_id
                                    || cached.result_id.as_ref().is_some_and(|id| {
                                        self.previous_results.get(&uri) == Some(id)
                                    }))
                        }))
            {
                continue;
            }
            apply_report(editor, &self.provider, uri.clone(), self.request_id, report);
            if full
                && editor
                    .pull_diagnostics
                    .reports
                    .get(&uri)
                    .and_then(|reports| reports.get(&self.provider))
                    .is_some_and(|cached| cached.request == self.request_id)
            {
                self.discarded_full.lock().remove(&uri);
            }
        }
        editor.redraw().request();
    }
}

/// Restart this provider's workspace pull after initialization or a server refresh.
pub fn request_workspace_diagnostics(editor: &mut Editor, server_id: LanguageServerId) {
    if !has_documents(editor, server_id) {
        return;
    }
    let Some(server) = editor.language_servers.get_by_id(server_id).cloned() else {
        return;
    };
    let Some(options) = server
        .capabilities()
        .diagnostic_provider
        .as_ref()
        .map(|options| match options {
            lsp::DiagnosticServerCapabilities::Options(options) => options,
            lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
                &options.diagnostic_options
            }
        })
    else {
        return;
    };
    if !options.workspace_diagnostics {
        return;
    }
    let provider = DiagnosticProvider::Lsp {
        server_id,
        identifier: options.identifier.as_deref().map(Into::into),
    };
    let previous_result_ids: Vec<_> = editor
        .pull_diagnostics
        .reports
        .iter()
        .filter_map(|(uri, reports)| {
            let cached = reports.get(&provider)?;
            cached.has_report.then_some(())?;
            Some(lsp::PreviousResultId {
                uri: uri.to_url().ok()?.as_str().parse().ok()?,
                value: cached.result_id.clone()?,
            })
        })
        .collect();
    let previous_results = previous_result_ids
        .iter()
        .filter_map(|previous| Some((previous.uri.to_editor_uri().ok()?, previous.value.clone())))
        .collect();
    let Some(mut stream) = server.workspace_diagnostics(previous_result_ids) else {
        return;
    };
    let cancel = editor
        .pull_diagnostics
        .workspace
        .entry(server_id)
        .or_default()
        .restart();
    editor.pull_diagnostics.next_request += 1;
    let request = Arc::new(WorkspacePull {
        server: Arc::downgrade(&server),
        provider,
        request_id: editor.pull_diagnostics.next_request,
        documents: editor
            .documents()
            .filter_map(|doc| {
                Some((
                    doc.uri()?,
                    OpenDocument {
                        id: doc.id(),
                        version: doc.version(),
                        generation: doc.pull_diagnostics.generation,
                    },
                ))
            })
            .collect(),
        cancel,
        previous_results,
        discarded_full: Default::default(),
    });
    let callbacks = editor.handlers.pull_diagnostics.callbacks.clone();
    tokio::spawn(async move {
        let mut retry = true;
        while let Some(Some(result)) = cancelable_future(stream.next(), &request.cancel).await {
            match result {
                Ok(report) => {
                    retry = true;
                    let request = request.clone();
                    callbacks
                        .send(move |editor| request.apply(editor, report))
                        .await;
                }
                Err(error) => {
                    retry = matches!(&error, lsp_client::Error::Rpc(error)
                        if error.code.code() == lsp::error_codes::SERVER_CANCELLED
                            && error.data.as_ref().and_then(|data| serde_json::from_value::<lsp::DiagnosticServerCancellationData>(data.clone()).ok()).is_none_or(|data| data.retrigger_request));
                    if retry {
                        log::debug!("Workspace diagnostic request canceled: {error}");
                    } else {
                        log::error!("Workspace diagnostic request failed: {error}");
                    }
                }
            }
        }
        drop(stream);
        // Servers may finish even an empty workspace report immediately. Delay
        // re-triggering so these servers cannot cause a busy request loop.
        if retry
            && cancelable_future(tokio::time::sleep(Duration::from_secs(1)), &request.cancel)
                .await
                .is_some()
        {
            callbacks
                .send(move |editor| {
                    if request.is_current(editor) {
                        request_workspace_diagnostics(editor, server_id);
                    }
                })
                .await;
        }
    });
}

pub(super) fn cancel_unused(editor: &mut Editor) {
    let unused: Vec<_> = editor
        .pull_diagnostics
        .workspace
        .keys()
        .copied()
        .filter(|&server| !has_documents(editor, server))
        .collect();
    for server in unused {
        editor.pull_diagnostics.workspace.remove(&server);
    }
}

pub(super) fn request_if_idle(editor: &mut Editor) {
    let servers: std::collections::HashSet<_> = editor
        .documents()
        .flat_map(|doc| {
            doc.language_servers_with_configured_feature(LanguageServerFeature::PullDiagnostics)
                .map(|server| server.id())
        })
        .collect();
    for server in servers {
        if !editor
            .pull_diagnostics
            .workspace
            .get(&server)
            .is_some_and(event::TaskController::is_running)
        {
            request_workspace_diagnostics(editor, server);
        }
    }
}

pub(super) fn document_opened(editor: &mut Editor, document: DocumentId) {
    request_if_idle(editor);
    let Some(doc) = editor.document(document) else {
        return;
    };
    // Direct pulls own their document's freshness. Other workspace providers
    // need a new snapshot when a previously unopened target gains a buffer.
    let servers: Vec<_> = editor
        .pull_diagnostics
        .workspace
        .keys()
        .copied()
        .filter(|&server| {
            !doc.language_servers_with_feature(LanguageServerFeature::PullDiagnostics)
                .any(|provider| provider.id() == server)
        })
        .collect();
    for server in servers {
        request_workspace_diagnostics(editor, server);
    }
}
