//! Workspace diagnostic streams keep partial reports in wire order with completion.

use std::{future::Future, pin::Pin};

use tokio::sync::mpsc::{self, UnboundedReceiver};

use super::Client;
use crate::{lsp, Result};

type Response = Pin<Box<dyn Future<Output = Result<lsp::WorkspaceDiagnosticReportResult>> + Send>>;

/// One workspace pull. Dropping the stream cancels its request and subscription.
pub struct WorkspaceDiagnosticRequest {
    response: Option<Response>,
    partial: UnboundedReceiver<serde_json::Value>,
}

impl WorkspaceDiagnosticRequest {
    /// Partial batches are delivered before the final response, even when both
    /// have already arrived. A malformed partial batch does not end the request.
    pub async fn next(&mut self) -> Option<Result<lsp::WorkspaceDiagnosticReport>> {
        let response = self.response.as_mut()?;
        tokio::select! {
            biased;
            Some(value) = self.partial.recv() => Some(serde_json::from_value(value).map_err(Into::into)),
            result = response => {
                self.response = None;
                Some(result.map(|result| match result {
                    lsp::WorkspaceDiagnosticReportResult::Report(report) => report,
                    lsp::WorkspaceDiagnosticReportResult::Partial(report) => lsp::WorkspaceDiagnosticReport { items: report.items },
                }))
            }
        }
    }
}

impl Client {
    /// Start the server's workspace pull with a request-owned partial-result token.
    pub fn workspace_diagnostics(
        &self,
        previous_result_ids: Vec<lsp::PreviousResultId>,
    ) -> Option<WorkspaceDiagnosticRequest> {
        let options = match self.capabilities().diagnostic_provider.as_ref()? {
            lsp::DiagnosticServerCapabilities::Options(options) => options,
            lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
                &options.diagnostic_options
            }
        };
        if !options.workspace_diagnostics {
            return None;
        }
        let id = self.next_request_id();
        let token = format!("{}{id}", crate::transport::PARTIAL_RESULT_TOKEN_PREFIX);
        let (sender, partial) = mpsc::unbounded_channel();
        let params = lsp::WorkspaceDiagnosticParams {
            identifier: options.identifier.clone(),
            previous_result_ids,
            work_done_progress_params: Default::default(),
            partial_result_params: lsp::PartialResultParams {
                partial_result_token: Some(lsp::ProgressToken::String(token.clone())),
            },
        };
        // The spec permits a server to keep this pull open indefinitely. Local
        // cancellation and transport shutdown still stop its response future.
        let response = self
            .pending_requests
            .request_with_partial::<lsp::request::WorkspaceDiagnosticRequest>(
                &self.server_tx,
                id,
                &params,
                u32::MAX.into(),
                Some((token, sender)),
            );
        Some(WorkspaceDiagnosticRequest {
            response: Some(Box::pin(response)),
            partial,
        })
    }
}
