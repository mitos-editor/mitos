//! Request registration, completion, and cancellation shared by client and I/O tasks.
use super::Payload;
use crate::{jsonrpc, lsp, Error, Result};
use parking_lot::Mutex;
use serde_json::Value;
use std::{collections::HashMap, future::Future, sync::Arc};
use tokio::sync::{
    mpsc::{UnboundedSender, WeakUnboundedSender},
    oneshot, Notify,
};

/// Shared with request futures so dropping one never waits for the writer task.
#[derive(Debug)]
pub(crate) struct PendingRequests {
    // None closes registration atomically with draining outstanding requests.
    requests: Mutex<Option<HashMap<jsonrpc::Id, PendingRequest>>>,
    closed: Notify,
}

#[derive(Debug)]
struct PendingRequest {
    sender: oneshot::Sender<Result<Value>>,
    started: bool,
}

impl Default for PendingRequests {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Some(HashMap::default())),
            closed: Notify::new(),
        }
    }
}

impl PendingRequests {
    pub(crate) fn request<R: lsp::request::Request>(
        self: &Arc<Self>,
        sender: &UnboundedSender<Payload>,
        id: jsonrpc::Id,
        params: &R::Params,
        timeout_secs: u64,
    ) -> impl Future<Output = Result<R::Result>> + use<R>
    where
        R::Params: serde::Serialize,
    {
        let cancellation = RequestCancellation {
            sender: sender.downgrade(),
            pending_requests: self.clone(),
            id: id.clone(),
        };
        // Queue eagerly to preserve ordering with document-change notifications.
        // The guard also covers futures dropped before their first poll.
        let response = serde_json::to_value(params)
            .map_err(Error::from)
            .and_then(|params| {
                let request = jsonrpc::MethodCall {
                    jsonrpc: Some(jsonrpc::Version::V2),
                    id: id.clone(),
                    method: R::METHOD.to_owned(),
                    params: params.into(),
                };
                self.enqueue(sender, request)
            });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        async move {
            let _cancellation = cancellation;
            let response = tokio::time::timeout_at(deadline, response?)
                .await
                .map_err(|_| Error::Timeout(id))?
                .map_err(|_| Error::StreamClosed)?;
            response.and_then(|value| serde_json::from_value(value).map_err(Into::into))
        }
    }

    /// Queue an intentional fire-and-forget request, preserving its transmission
    /// even though there is no response future to await.
    pub(crate) fn send_fire_and_forget(
        &self,
        sender: &UnboundedSender<Payload>,
        request: jsonrpc::MethodCall,
    ) -> Result<()> {
        self.enqueue(sender, request).map(drop)
    }

    fn enqueue(
        &self,
        sender: &UnboundedSender<Payload>,
        request: jsonrpc::MethodCall,
    ) -> Result<oneshot::Receiver<Result<Value>>> {
        let id = request.id.clone();
        let receiver = self.register(id.clone())?;
        if sender.send(Payload::Request(request)).is_err() {
            self.cancel(&id);
            return Err(Error::StreamClosed);
        }
        Ok(receiver)
    }

    pub(super) fn complete(&self, output: jsonrpc::Output, language_server_name: &str) {
        let (id, result) = match output {
            jsonrpc::Output::Success(jsonrpc::Success { id, result, .. }) => (id, Ok(result)),
            jsonrpc::Output::Failure(jsonrpc::Failure { id, error, .. }) => (id, Err(error.into())),
        };

        let Some(request) = self.remove(&id) else {
            log::debug!(
                "Discarding Language Server response without a request (id={:?}) {:?}",
                id,
                result
            );
            return;
        };
        if let Err(err) = &result {
            log::log!(
                crate::error_log_level(err),
                "{language_server_name} <- {err}"
            );
        }
        if request.sender.send(result).is_err() {
            log::debug!(
                "Response receiver already closed (id={id:?}), possibly a fire-and-forget shutdown"
            );
        }
    }

    fn register(&self, id: jsonrpc::Id) -> Result<oneshot::Receiver<Result<Value>>> {
        let (sender, receiver) = oneshot::channel();
        self.requests
            .lock()
            .as_mut()
            .ok_or(Error::StreamClosed)?
            .insert(
                id,
                PendingRequest {
                    sender,
                    started: false,
                },
            );
        Ok(receiver)
    }

    /// Removes the response handler and reports whether transmission has started.
    fn cancel(&self, id: &jsonrpc::Id) -> Option<bool> {
        self.remove(id).map(|request| request.started)
    }

    fn remove(&self, id: &jsonrpc::Id) -> Option<PendingRequest> {
        self.requests.lock().as_mut()?.remove(id)
    }

    pub(super) fn start(&self, id: &jsonrpc::Id) -> bool {
        let mut requests = self.requests.lock();
        let Some(request) = requests.as_mut().and_then(|requests| requests.get_mut(id)) else {
            return false;
        };
        request.started = true;
        true
    }

    /// Close registration and report whether this call performed the transition.
    pub(super) fn close(&self) -> bool {
        // Dropping the senders wakes every waiter with StreamClosed.
        let pending = self.requests.lock().take();
        let was_open = pending.is_some();
        drop(pending);
        self.closed.notify_waiters();
        was_open
    }

    pub(super) async fn closed(&self) {
        let notified = self.closed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.requests.lock().is_some() {
            notified.await;
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.requests.lock().as_ref().map_or(0, HashMap::len)
    }
}

/// Cancels local response tracking synchronously; the writer sends the protocol
/// notification in order after any request frame already being written.
struct RequestCancellation {
    sender: WeakUnboundedSender<Payload>,
    pending_requests: Arc<PendingRequests>,
    id: jsonrpc::Id,
}

impl Drop for RequestCancellation {
    fn drop(&mut self) {
        if let Some(notify_server) = self.pending_requests.cancel(&self.id)
            && let Some(sender) = self.sender.upgrade()
        {
            let _ = sender.send(Payload::Cancel {
                id: self.id.clone(),
                notify_server,
            });
        }
    }
}
