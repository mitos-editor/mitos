mod requests;
pub(crate) use requests::PendingRequests;

use crate::{
    jsonrpc,
    lsp::{self, notification::Notification as _},
    Error, LanguageServerId, Result,
};
use anyhow::Context;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader, BufWriter},
    sync::{
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        oneshot, Notify,
    },
};

#[derive(Debug)]
pub enum Payload {
    Request(jsonrpc::MethodCall),
    Cancel {
        id: jsonrpc::Id,
        notify_server: bool,
    },
    Notification(jsonrpc::Notification),
    Response(jsonrpc::Output),
}

/// A type representing all possible values sent from the server to the client.
#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
enum ServerMessage {
    /// A regular JSON-RPC request output (single response).
    Output(jsonrpc::Output),
    /// A JSON-RPC request or notification.
    Call(jsonrpc::Call),
}

/// Owns one connection task. Dropping it closes all IO even if senders survive.
#[derive(Debug)]
pub struct Transport {
    _shutdown: oneshot::Sender<()>,
}

#[derive(Debug)]
struct TransportState {
    id: LanguageServerId,
    name: String,
    pending_requests: Arc<PendingRequests>,
    shutdown_requested: AtomicBool,
    inject_tx: UnboundedSender<Payload>,
    /// Notified once the `exit` notification has been flushed to the server's stdin
    shutdown_flushed: Arc<Notify>,
}

impl Drop for TransportState {
    fn drop(&mut self) {
        // Also drain waits if the runtime drops the task before its normal terminal transition.
        self.pending_requests.close();
    }
}

/// The channels and shared state established when a transport starts.
pub(crate) struct Connection {
    pub transport: Transport,
    pub receiver: UnboundedReceiver<(LanguageServerId, jsonrpc::Call)>,
    pub sender: UnboundedSender<Payload>,
    pub initialized: Arc<Notify>,
    pub shutdown_flushed: Arc<Notify>,
    pub requests: Arc<PendingRequests>,
}

impl Transport {
    pub(crate) fn start(
        server_stdout: BufReader<impl AsyncRead + Unpin + Send + 'static>,
        server_stdin: BufWriter<impl AsyncWrite + Unpin + Send + 'static>,
        server_stderr: BufReader<impl AsyncRead + Unpin + Send + 'static>,
        id: LanguageServerId,
        name: String,
    ) -> Connection {
        let (client_tx, rx) = unbounded_channel();
        let (tx, client_rx) = unbounded_channel();
        let (inject_tx, inject_rx) = unbounded_channel();
        let notify = Arc::new(Notify::new());
        let shutdown_flushed = Arc::new(Notify::new());
        let pending_requests = Arc::new(PendingRequests::default());

        let transport = TransportState {
            id,
            name,
            pending_requests: pending_requests.clone(),
            shutdown_requested: AtomicBool::new(false),
            inject_tx,
            shutdown_flushed: shutdown_flushed.clone(),
        };

        let (shutdown, canceled) = oneshot::channel();
        let initialize_notify = notify.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                // Give both IO directions progress under sustained incoming traffic.
                _ = canceled => Ok(()),
                _ = client_tx.closed() => Ok(()),
                result = transport.recv(server_stdout, &client_tx) => result,
                result = transport.send_messages(server_stdin, &client_tx, client_rx, inject_rx, initialize_notify) => result,
                _ = async {
                    transport.err(server_stderr).await;
                    // Stderr may close independently of the protocol streams.
                    std::future::pending::<()>().await;
                } => unreachable!(),
            };
            if let Err(error) = result
                && !matches!(error, Error::StreamClosed)
            {
                error!(
                    "Exiting {} after unexpected error: {error:?}",
                    transport.name
                );
            }
            // All IO futures have been dropped; close registration and wake every waiter once.
            transport.terminate(&client_tx);
        });

        Connection {
            transport: Transport {
                _shutdown: shutdown,
            },
            receiver: rx,
            sender: tx,
            initialized: notify,
            shutdown_flushed,
            requests: pending_requests,
        }
    }
}

impl TransportState {
    async fn recv_server_message(
        reader: &mut (impl AsyncBufRead + Unpin + Send),
        buffer: &mut String,
        content: &mut Vec<u8>,
        language_server_name: &str,
    ) -> Result<ServerMessage> {
        protocol_io::read_frame(reader, buffer, content).await?;
        let msg = std::str::from_utf8(content).context("invalid utf8 from server")?;

        info!("{language_server_name} <- {msg}");

        // NOTE: We avoid using `?` here, since it would return early on error
        // and skip clearing `content`. By returning the result directly instead,
        // we ensure `content.clear()` is always called.
        let output = sonic_rs::from_slice(content).map_err(Into::into);

        content.clear();

        output
    }

    async fn recv_server_error(
        err: &mut (impl AsyncBufRead + Unpin + Send),
        buffer: &mut String,
        language_server_name: &str,
    ) -> Result<()> {
        buffer.clear();
        if err.read_line(buffer).await? == 0 {
            return Err(Error::StreamClosed);
        };
        error!("{language_server_name} err <- {buffer:?}");

        Ok(())
    }

    async fn send_payload_to_server<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        server_stdin: &mut BufWriter<W>,
        payload: Payload,
        json: &mut Vec<u8>,
    ) -> Result<()> {
        json.clear();
        match payload {
            Payload::Request(value) => {
                if !self.pending_requests.start(&value.id) {
                    return Ok(());
                }
                sonic_rs::to_writer(&mut *json, &value)?
            }
            Payload::Cancel { id, notify_server } => {
                if !notify_server {
                    return Ok(());
                }
                sonic_rs::to_writer(
                    &mut *json,
                    &jsonrpc::Notification {
                        jsonrpc: Some(jsonrpc::Version::V2),
                        method: lsp::notification::Cancel::METHOD.into(),
                        params: jsonrpc::Params::Map(serde_json::Map::from_iter([(
                            "id".into(),
                            serde_json::to_value(id)?,
                        )])),
                    },
                )?
            }
            Payload::Notification(value) => sonic_rs::to_writer(&mut *json, &value)?,
            Payload::Response(error) => sonic_rs::to_writer(&mut *json, &error)?,
        };
        self.send_bytes_to_server(server_stdin, json).await?;
        // Reuse ordinary messages without retaining an unusually large document.
        if json.capacity() > 4 * 1024 * 1024 {
            *json = Vec::new();
        }
        Ok(())
    }

    async fn send_bytes_to_server<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        server_stdin: &mut BufWriter<W>,
        request: &[u8],
    ) -> Result<()> {
        info!(
            "{} -> {}",
            self.name,
            std::str::from_utf8(request).expect("JSON is valid UTF-8")
        );

        protocol_io::write_frame(server_stdin, request).await?;
        Ok(())
    }

    fn process_server_message(
        &self,
        client_tx: &UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
        msg: ServerMessage,
    ) -> Result<()> {
        match msg {
            ServerMessage::Output(output) => {
                self.pending_requests.complete(output, &self.name);
            }
            ServerMessage::Call(jsonrpc::Call::MethodCall(ref method_call))
                if self.shutdown_requested.load(Ordering::Acquire) =>
            {
                // After mitos sends shutdown the application event loop is no longer
                // consuming server-to-client requests. Respond with null success so the
                // server is not left waiting for a reply before it sends the shutdown
                // response. Sending an error is intentionally avoided: servers based on
                // vscode-languageserver-node (including gopls) treat an error response to
                // client/registerCapability as fatal and abort rather than completing the
                // handshake.
                let _ = self
                    .inject_tx
                    .send(Payload::Response(jsonrpc::Output::Success(
                        jsonrpc::Success {
                            jsonrpc: Some(jsonrpc::Version::V2),
                            id: method_call.id.clone(),
                            result: serde_json::Value::Null,
                        },
                    )));
            }
            ServerMessage::Call(call) => {
                client_tx
                    .send((self.id, call))
                    .context("failed to send a message to server")?;
            }
        };
        Ok(())
    }

    fn terminate(&self, client_tx: &UnboundedSender<(LanguageServerId, jsonrpc::Call)>) {
        if self.pending_requests.close() {
            let _ = client_tx.send((
                self.id,
                jsonrpc::Call::Notification(jsonrpc::Notification {
                    jsonrpc: None,
                    method: lsp::notification::Exit::METHOD.into(),
                    params: jsonrpc::Params::None,
                }),
            ));
        }
    }

    async fn recv<R: AsyncRead + Unpin + Send>(
        &self,
        mut server_stdout: BufReader<R>,
        client_tx: &UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
    ) -> Result<()> {
        let mut recv_buffer = String::new();
        let mut content_buffer = Vec::new();
        loop {
            let message = Self::recv_server_message(
                &mut server_stdout,
                &mut recv_buffer,
                &mut content_buffer,
                &self.name,
            )
            .await?;
            self.process_server_message(client_tx, message)?;
            tokio::task::consume_budget().await;
        }
    }

    async fn err<R: AsyncRead + Unpin + Send>(&self, mut server_stderr: BufReader<R>) {
        let mut recv_buffer = String::new();
        loop {
            if let Err(err) =
                Self::recv_server_error(&mut server_stderr, &mut recv_buffer, &self.name).await
            {
                if !matches!(err, Error::StreamClosed) {
                    error!("{} err: <- {err:?}", self.name);
                }
                return;
            }
        }
    }

    async fn send_messages<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        mut server_stdin: BufWriter<W>,
        client_tx: &UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
        mut client_rx: UnboundedReceiver<Payload>,
        mut inject_rx: UnboundedReceiver<Payload>,
        initialize_notify: Arc<Notify>,
    ) -> Result<()> {
        let mut pending_messages: Vec<Payload> = Vec::new();
        let mut json = Vec::new();
        let mut is_pending = true;

        // Retain the notification future so initialization cannot lose a wakeup.
        let notified = initialize_notify.notified();
        tokio::pin!(notified);

        fn is_initialize(payload: &Payload) -> bool {
            use lsp::{
                notification::Initialized,
                request::{Initialize, Request},
            };
            match payload {
                Payload::Request(jsonrpc::MethodCall { method, .. })
                    if method == Initialize::METHOD =>
                {
                    true
                }
                Payload::Notification(jsonrpc::Notification { method, .. })
                    if method == Initialized::METHOD =>
                {
                    true
                }
                _ => false,
            }
        }

        fn is_shutdown(payload: &Payload) -> bool {
            use lsp::request::{Request, Shutdown};
            matches!(payload, Payload::Request(jsonrpc::MethodCall { method, .. }) if method == Shutdown::METHOD)
        }

        fn is_exit(payload: &Payload) -> bool {
            matches!(payload, Payload::Notification(jsonrpc::Notification { method, .. }) if method == lsp::notification::Exit::METHOD)
        }

        loop {
            tokio::select! {
                // Initialization precedes queued work; server replies must not
                // wait behind a sustained backlog of editor requests.
                biased;
                _ = &mut notified, if is_pending => {
                    is_pending = false;
                    // Notify the editor after initialization, before feature requests.
                    let notification = ServerMessage::Call(jsonrpc::Call::Notification(jsonrpc::Notification {
                        jsonrpc: None,
                        method: lsp::notification::Initialized::METHOD.to_string(),
                        params: jsonrpc::Params::None,
                    }));
                    self.process_server_message(client_tx, notification)?;
                    for msg in pending_messages.drain(..) {
                        self.send_payload_to_server(&mut server_stdin, msg, &mut json).await?;
                    }
                }
                msg = inject_rx.recv() => {
                    if let Some(msg) = msg {
                        self.send_payload_to_server(&mut server_stdin, msg, &mut json).await?;
                    }
                }
                msg = client_rx.recv() => {
                    let Some(msg) = msg else { break; };
                    if let Payload::Cancel { id, .. } = &msg {
                        // Cancellation is allowed during initialization. Drop queued
                        // requests now rather than retaining them until it completes.
                        pending_messages.retain(|msg| !matches!(msg, Payload::Request(request) if request.id == *id));
                    }
                    if is_pending && is_shutdown(&msg) {
                        info!("Language server not initialized, shutting down");
                        break;
                    }
                    if is_pending && !is_initialize(&msg) && !matches!(msg, Payload::Cancel { .. }) {
                        if !matches!(msg, Payload::Notification(_)) {
                            pending_messages.push(msg);
                        }
                        continue;
                    }
                    // Receiving must observe this before the server can respond
                    // with its own requests during the shutdown handshake.
                    if is_shutdown(&msg) {
                        self.shutdown_requested.store(true, Ordering::Release);
                    }
                    let is_exit = is_exit(&msg);
                    self.send_payload_to_server(&mut server_stdin, msg, &mut json).await?;
                    if is_exit {
                        self.shutdown_flushed.notify_one();
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
