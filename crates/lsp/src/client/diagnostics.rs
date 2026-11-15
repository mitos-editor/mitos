//! Workspace diagnostic streams keep partial reports in wire order with completion.

use super::{Client, PartialResultRequest, RequestUpdate};
use crate::{lsp, Result};

/// One workspace pull. Dropping the stream cancels its request and subscription.
pub struct WorkspaceDiagnosticRequest {
    request: PartialResultRequest<lsp::WorkspaceDiagnosticReport, lsp::WorkspaceDiagnosticReport>,
}

impl WorkspaceDiagnosticRequest {
    /// Partial batches are delivered before the final response, even when both
    /// have already arrived. A malformed partial batch does not end the request.
    pub async fn next(&mut self) -> Option<Result<lsp::WorkspaceDiagnosticReport>> {
        match self.request.next().await? {
            RequestUpdate::Partial(batch) => Some(batch),
            RequestUpdate::Response(response) => Some(response.map(Option::unwrap_or_default)),
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
        let params = lsp::WorkspaceDiagnosticParams {
            identifier: options.identifier.clone(),
            previous_result_ids,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        // The spec permits a server to keep this pull open indefinitely. Local
        // cancellation and transport shutdown still stop its response future.
        Some(WorkspaceDiagnosticRequest {
            request: self.partial_request::<lsp::request::WorkspaceDiagnosticRequest>(
                params,
                u32::MAX.into(),
            ),
        })
    }
}
