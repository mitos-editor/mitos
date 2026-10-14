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
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    process::{ChildStderr, ChildStdin, ChildStdout},
    sync::{
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        Notify,
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

#[derive(Debug)]
pub struct Transport {
    id: LanguageServerId,
    name: String,
    pending_requests: Arc<PendingRequests>,
    shutdown_requested: AtomicBool,
    inject_tx: UnboundedSender<Payload>,
    /// Notified once the `exit` notification has been flushed to the server's stdin
    shutdown_flushed: Arc<Notify>,
}

/// The channels and shared state established when a transport starts.
pub(crate) struct Connection {
    pub receiver: UnboundedReceiver<(LanguageServerId, jsonrpc::Call)>,
    pub sender: UnboundedSender<Payload>,
    pub initialized: Arc<Notify>,
    pub shutdown_flushed: Arc<Notify>,
    pub requests: Arc<PendingRequests>,
}

impl Transport {
    pub(crate) fn start(
        server_stdout: BufReader<ChildStdout>,
        server_stdin: BufWriter<ChildStdin>,
        server_stderr: BufReader<ChildStderr>,
        id: LanguageServerId,
        name: String,
    ) -> Connection {
        let (client_tx, rx) = unbounded_channel();
        let (tx, client_rx) = unbounded_channel();
        let (inject_tx, inject_rx) = unbounded_channel();
        let notify = Arc::new(Notify::new());
        let shutdown_flushed = Arc::new(Notify::new());
        let pending_requests = Arc::new(PendingRequests::default());

        let transport = Self {
            id,
            name,
            pending_requests: pending_requests.clone(),
            shutdown_requested: AtomicBool::new(false),
            inject_tx,
            shutdown_flushed: shutdown_flushed.clone(),
        };

        let transport = Arc::new(transport);

        tokio::spawn(Self::recv(
            transport.clone(),
            server_stdout,
            client_tx.clone(),
        ));
        tokio::spawn(Self::err(transport.clone(), server_stderr));
        tokio::spawn(Self::send(
            transport,
            server_stdin,
            client_tx,
            client_rx,
            inject_rx,
            notify.clone(),
        ));

        Connection {
            receiver: rx,
            sender: tx,
            initialized: notify,
            shutdown_flushed,
            requests: pending_requests,
        }
    }

    async fn recv_server_message(
        reader: &mut (impl AsyncBufRead + Unpin + Send),
        buffer: &mut String,
        content: &mut Vec<u8>,
        language_server_name: &str,
    ) -> Result<ServerMessage> {
        let mut content_length = None;
        loop {
            buffer.clear();
            if reader.read_line(buffer).await? == 0 {
                return Err(Error::StreamClosed);
            }

            // debug!("<- header {:?}", buffer);

            if buffer == "\r\n" {
                // look for an empty CRLF line
                break;
            }

            let header = buffer.trim();

            let parts = header.split_once(": ");

            match parts {
                Some(("Content-Length", value)) => {
                    content_length = Some(value.parse().context("invalid content length")?);
                }
                Some((_, _)) => {}
                None => {
                    // Workaround: Some non-conformant language servers will output logging and other garbage
                    // into the same stream as JSON-RPC messages. This can also happen from shell scripts that spawn
                    // the server. Skip such lines and log a warning.

                    // warn!("Failed to parse header: {:?}", header);
                }
            }
        }

        let content_length = content_length.context("missing content length")?;
        content.resize(content_length, 0);
        reader.read_exact(content).await?;
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
    ) -> Result<()> {
        //TODO: reuse string
        let json = match payload {
            Payload::Request(value) => {
                if !self.pending_requests.start(&value.id) {
                    return Ok(());
                }
                serde_json::to_string(&value)?
            }
            Payload::Cancel { id, notify_server } => {
                if !notify_server {
                    return Ok(());
                }
                serde_json::to_string(&jsonrpc::Notification {
                    jsonrpc: Some(jsonrpc::Version::V2),
                    method: lsp::notification::Cancel::METHOD.into(),
                    params: jsonrpc::Params::Map(serde_json::Map::from_iter([(
                        "id".into(),
                        serde_json::to_value(id)?,
                    )])),
                })?
            }
            Payload::Notification(value) => serde_json::to_string(&value)?,
            Payload::Response(error) => serde_json::to_string(&error)?,
        };
        self.send_string_to_server(server_stdin, json).await
    }

    async fn send_string_to_server<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        server_stdin: &mut BufWriter<W>,
        request: String,
    ) -> Result<()> {
        info!("{} -> {request}", self.name);

        // send the headers
        server_stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", request.len()).as_bytes())
            .await?;

        // send the body
        server_stdin.write_all(request.as_bytes()).await?;

        server_stdin.flush().await?;

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

    async fn recv<R: tokio::io::AsyncRead + Unpin + Send>(
        transport: Arc<Self>,
        mut server_stdout: BufReader<R>,
        client_tx: UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
    ) {
        let mut recv_buffer = String::new();
        let mut content_buffer = Vec::new();
        let closed = transport.pending_requests.closed();
        tokio::pin!(closed);
        loop {
            let message = tokio::select! {
                biased;
                _ = &mut closed => return,
                message = Self::recv_server_message(
                    &mut server_stdout, &mut recv_buffer, &mut content_buffer, &transport.name,
                ) => message,
            };
            let result = message.and_then(|msg| transport.process_server_message(&client_tx, msg));
            if let Err(err) = result {
                if !matches!(err, Error::StreamClosed) {
                    error!("Exiting {} after unexpected error: {err:?}", transport.name);
                }
                break;
            }
        }
        transport.terminate(&client_tx);
    }

    async fn err(transport: Arc<Self>, mut server_stderr: BufReader<ChildStderr>) {
        let mut recv_buffer = String::new();
        let closed = transport.pending_requests.closed();
        tokio::pin!(closed);
        loop {
            let result = tokio::select! {
                biased;
                _ = &mut closed => return,
                result = Self::recv_server_error(&mut server_stderr, &mut recv_buffer, &transport.name) => result,
            };
            if let Err(err) = result {
                if !matches!(err, Error::StreamClosed) {
                    error!("{} err: <- {err:?}", transport.name);
                }
                break;
            }
        }
    }

    async fn send<W: tokio::io::AsyncWrite + Unpin>(
        transport: Arc<Self>,
        server_stdin: BufWriter<W>,
        client_tx: UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
        client_rx: UnboundedReceiver<Payload>,
        inject_rx: UnboundedReceiver<Payload>,
        initialize_notify: Arc<Notify>,
    ) {
        tokio::select! {
            biased;
            _ = transport.pending_requests.closed() => {},
            result = Self::send_messages(
                transport.clone(), server_stdin, client_tx.clone(), client_rx, inject_rx, initialize_notify,
            ) => {
                if let Err(err) = result {
                    error!("{} failed to write to language server: {err}", transport.name);
                }
            }
        }
        // A failed write may leave a partial frame. Stop using this stream and
        // wake every waiter, including requests still in the outgoing queue.
        transport.terminate(&client_tx);
    }

    async fn send_messages<W: tokio::io::AsyncWrite + Unpin>(
        transport: Arc<Self>,
        mut server_stdin: BufWriter<W>,
        client_tx: UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
        mut client_rx: UnboundedReceiver<Payload>,
        mut inject_rx: UnboundedReceiver<Payload>,
        initialize_notify: Arc<Notify>,
    ) -> Result<()> {
        let mut pending_messages: Vec<Payload> = Vec::new();
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
                    transport.process_server_message(&client_tx, notification)?;
                    for msg in pending_messages.drain(..) {
                        transport.send_payload_to_server(&mut server_stdin, msg).await?;
                    }
                }
                msg = inject_rx.recv() => {
                    if let Some(msg) = msg {
                        transport.send_payload_to_server(&mut server_stdin, msg).await?;
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
                    // The receive task must see this before the server can respond
                    // with its own requests during the shutdown handshake.
                    if is_shutdown(&msg) {
                        transport.shutdown_requested.store(true, Ordering::Release);
                    }
                    let is_exit = is_exit(&msg);
                    transport.send_payload_to_server(&mut server_stdin, msg).await?;
                    if is_exit {
                        transport.shutdown_flushed.notify_one();
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
