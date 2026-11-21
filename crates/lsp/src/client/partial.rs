//! Typed, request-owned partial results. Accumulation follows each method's wire shape.

use std::{future::Future, marker::PhantomData, pin::Pin};

use futures_util::FutureExt as _;
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver};

mod responses;
#[cfg(test)]
mod tests;

use super::Client;
use crate::{lsp, Error, Result};
use responses::PartialResponse;

type Response = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;

/// One update from a request. Only `Response` ends the request; a malformed
/// partial can be reported without losing later batches or the final response.
pub enum RequestUpdate<T, P> {
    /// Provisional items. Consumers must discard them if the response fails.
    Partial(Result<P>),
    /// The terminal response; `None` represents the empty final response used
    /// after streaming. This is separate from a method's nullable result type.
    Response(Result<Option<T>>),
}

/// A partial-result subscription and its final response share one lifetime.
/// Dropping this value cancels both, including before the first poll.
pub struct PartialResultRequest<T, P> {
    response: Option<Response>,
    partial: UnboundedReceiver<Value>,
    finished: Option<Result<Value>>,
    decode_partial: fn(Value) -> Result<P>,
    decode_response: fn(Value) -> Result<Option<T>>,
}

impl<T, P> PartialResultRequest<T, P> {
    /// Batches queued before completion are drained in wire order. Polling the
    /// response first also enforces deadlines when a server streams continuously.
    pub async fn next(&mut self) -> Option<RequestUpdate<T, P>> {
        loop {
            if let Some(response) = &mut self.response
                && let Some(result) = response.as_mut().now_or_never()
            {
                self.response = None;
                self.finished = Some(result);
            }
            if self.finished.is_some() {
                if let Ok(value) = self.partial.try_recv() {
                    return Some(RequestUpdate::Partial((self.decode_partial)(value)));
                }
                return self
                    .finished
                    .take()
                    .map(|result| RequestUpdate::Response(result.and_then(self.decode_response)));
            }
            let response = self.response.as_mut()?;
            tokio::select! {
                biased;
                result = response => {
                    self.response = None;
                    self.finished = Some(result);
                }
                Some(value) = self.partial.recv() => {
                    return Some(RequestUpdate::Partial((self.decode_partial)(value)));
                }
            }
        }
    }
}

// Decode the final response after draining progress. Some methods (notably
// document diagnostics) put their primary result in progress and finish with null.
struct Raw<R>(PhantomData<R>);
impl<R: lsp::request::Request> lsp::request::Request for Raw<R> {
    type Params = R::Params;
    type Result = Value;
    const METHOD: &'static str = R::METHOD;
}

pub(crate) trait PartialParams {
    fn partial_params(&mut self) -> &mut lsp::PartialResultParams;
}

macro_rules! partial_params {
    ($($ty:ty),* $(,)?) => {$(
        impl PartialParams for $ty {
            fn partial_params(&mut self) -> &mut lsp::PartialResultParams {
                &mut self.partial_result_params
            }
        }
    )*};
}
partial_params!(
    lsp::FoldingRangeParams,
    lsp::SelectionRangeParams,
    lsp::CompletionParams,
    lsp::InlineCompletionParams,
    lsp::DocumentColorParams,
    lsp::DocumentLinkParams,
    lsp::DocumentDiagnosticParams,
    lsp::DocumentHighlightParams,
    lsp::GotoDefinitionParams,
    lsp::ReferenceParams,
    lsp::DocumentSymbolParams,
    lsp::WorkspaceSymbolParams,
    lsp::CodeActionParams,
    lsp::SemanticTokensParams,
    lsp::SemanticTokensRangeParams,
    lsp::SemanticTokensDeltaParams,
    lsp::CodeLensParams,
    lsp::WorkspaceDiagnosticParams,
);

impl Client {
    pub(crate) fn partial_request<R>(
        &self,
        params: R::Params,
        timeout: u64,
    ) -> PartialResultRequest<R::Result, <R::Result as PartialResponse>::Partial>
    where
        R: lsp::request::Request + 'static,
        R::Params: PartialParams,
        R::Result: PartialResponse,
    {
        start_request::<R>(
            &self.pending_requests,
            &self.server_tx,
            self.next_request_id(),
            params,
            timeout,
        )
    }

    pub(crate) fn call_partial<R>(
        &self,
        params: R::Params,
    ) -> impl Future<Output = Result<R::Result>> + use<R>
    where
        R: lsp::request::Request + 'static,
        R::Params: PartialParams,
        R::Result: PartialResponse,
    {
        // Construct outside the async block: enqueue and cancellation are eager.
        let request = self.partial_request::<R>(params, self.req_timeout);
        collect::<R::Result>(request)
    }
}

pub(crate) fn start_request<R>(
    pending: &std::sync::Arc<crate::transport::PendingRequests>,
    sender: &tokio::sync::mpsc::UnboundedSender<crate::transport::Payload>,
    id: crate::jsonrpc::Id,
    mut params: R::Params,
    timeout: u64,
) -> PartialResultRequest<R::Result, <R::Result as PartialResponse>::Partial>
where
    R: lsp::request::Request + 'static,
    R::Params: PartialParams,
    R::Result: PartialResponse,
{
    let token = format!("{}{id}", crate::transport::PARTIAL_RESULT_TOKEN_PREFIX);
    params.partial_params().partial_result_token = Some(lsp::ProgressToken::String(token.clone()));
    let (partial_sender, partial) = mpsc::unbounded_channel();
    let response = pending.request_with_partial::<Raw<R>>(
        sender,
        id,
        &params,
        timeout,
        Some((token, partial_sender)),
    );
    PartialResultRequest {
        response: Some(Box::pin(response)),
        partial,
        finished: None,
        decode_partial: R::Result::decode_partial,
        decode_response: R::Result::decode_response,
    }
}

pub(crate) async fn collect<T: PartialResponse>(
    mut request: PartialResultRequest<T, T::Partial>,
) -> Result<T> {
    let mut state = T::State::default();
    while let Some(update) = request.next().await {
        match update {
            RequestUpdate::Partial(batch) => T::append(&mut state, batch?)?,
            // An error invalidates all accumulated results, including cancellation.
            RequestUpdate::Response(response) => return T::finish(state, response?),
        }
    }
    Err(Error::StreamClosed)
}
