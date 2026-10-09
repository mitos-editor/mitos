use crate::{
    jsonrpc,
    lsp::{self, notification::Notification as _},
    Error, LanguageServerId, Result,
};
use anyhow::Context;
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    process::{ChildStderr, ChildStdin, ChildStdout},
    sync::{
        mpsc::{unbounded_channel, Sender, UnboundedReceiver, UnboundedSender},
        Mutex, Notify, OwnedSemaphorePermit,
    },
};

#[derive(Debug)]
pub enum Payload {
    Request {
        chan: Sender<Result<Value>>,
        value: jsonrpc::MethodCall,
        permit: Option<OwnedSemaphorePermit>,
    },
    Cancel(jsonrpc::Id),
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
    pending_requests: Mutex<HashMap<jsonrpc::Id, PendingRequest>>,
    shutdown_requested: AtomicBool,
    inject_tx: UnboundedSender<Payload>,
    /// Notified once the `exit` notification has been flushed to the server's stdin
    shutdown_flushed: Arc<Notify>,
}

#[derive(Debug)]
struct PendingRequest {
    sender: Sender<Result<Value>>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl Transport {
    #[allow(clippy::type_complexity)]
    pub fn start(
        server_stdout: BufReader<ChildStdout>,
        server_stdin: BufWriter<ChildStdin>,
        server_stderr: BufReader<ChildStderr>,
        id: LanguageServerId,
        name: String,
    ) -> (
        UnboundedReceiver<(LanguageServerId, jsonrpc::Call)>,
        UnboundedSender<Payload>,
        Arc<Notify>,
        Arc<Notify>,
    ) {
        let (client_tx, rx) = unbounded_channel();
        let (tx, client_rx) = unbounded_channel();
        let (inject_tx, inject_rx) = unbounded_channel();
        let notify = Arc::new(Notify::new());
        let shutdown_flushed = Arc::new(Notify::new());

        let transport = Self {
            id,
            name,
            pending_requests: Mutex::new(HashMap::default()),
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

        (rx, tx, notify, shutdown_flushed)
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
            Payload::Request {
                chan,
                value,
                permit,
            } => {
                self.pending_requests.lock().await.insert(
                    value.id.clone(),
                    PendingRequest {
                        sender: chan,
                        _permit: permit,
                    },
                );
                serde_json::to_string(&value)?
            }
            Payload::Cancel(id) => {
                // Release local admission before awaiting another server write.
                // A late response has no registered receiver and cannot revive it.
                if self.pending_requests.lock().await.remove(&id).is_none() {
                    return Ok(());
                }
                serde_json::to_string(&jsonrpc::Notification {
                    jsonrpc: Some(jsonrpc::Version::V2),
                    method: "$/cancelRequest".into(),
                    params: jsonrpc::Params::Map(serde_json::Map::from_iter([(
                        "id".into(),
                        serde_json::to_value(id)?,
                    )])),
                })?
            }
            Payload::Notification(value) => serde_json::to_string(&value)?,
            Payload::Response(error) => serde_json::to_string(&error)?,
        };
        self.send_string_to_server(server_stdin, json, &self.name)
            .await
    }

    async fn send_string_to_server<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        server_stdin: &mut BufWriter<W>,
        request: String,
        language_server_name: &str,
    ) -> Result<()> {
        info!("{language_server_name} -> {request}");

        // send the headers
        server_stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", request.len()).as_bytes())
            .await?;

        // send the body
        server_stdin.write_all(request.as_bytes()).await?;

        server_stdin.flush().await?;

        Ok(())
    }

    async fn process_server_message(
        &self,
        client_tx: &UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
        msg: ServerMessage,
        language_server_name: &str,
    ) -> Result<()> {
        match msg {
            ServerMessage::Output(output) => {
                self.process_request_response(output, language_server_name)
                    .await?
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

    async fn close_pending(&self) {
        let pending: Vec<_> = self.pending_requests.lock().await.drain().collect();
        for (id, request) in pending {
            if request.sender.send(Err(Error::StreamClosed)).await.is_err() {
                log::debug!("Could not close request on a closed channel (id={id:?})");
            }
        }
    }

    async fn process_request_response(
        &self,
        output: jsonrpc::Output,
        language_server_name: &str,
    ) -> Result<()> {
        let (id, result) = match output {
            jsonrpc::Output::Success(jsonrpc::Success { id, result, .. }) => (id, Ok(result)),
            jsonrpc::Output::Failure(jsonrpc::Failure { id, error, .. }) => {
                error!("{language_server_name} <- {error}");
                (id, Err(error.into()))
            }
        };

        let tx = self.pending_requests.lock().await.remove(&id);
        if let Some(tx) = tx {
            match tx.sender.send(result).await {
                Ok(_) => (),
                Err(_) => log::debug!(
                    "Tried sending response into a closed channel (id={:?}), likely a fire-and-forget shutdown",
                    id
                ),
            };
        } else {
            log::error!(
                "Discarding Language Server response without a request (id={:?}) {:?}",
                id,
                result
            );
        }

        Ok(())
    }

    async fn recv(
        transport: Arc<Self>,
        mut server_stdout: BufReader<ChildStdout>,
        client_tx: UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
    ) {
        let mut recv_buffer = String::new();
        let mut content_buffer = Vec::new();
        loop {
            match Self::recv_server_message(
                &mut server_stdout,
                &mut recv_buffer,
                &mut content_buffer,
                &transport.name,
            )
            .await
            {
                Ok(msg) => {
                    match transport
                        .process_server_message(&client_tx, msg, &transport.name)
                        .await
                    {
                        Ok(_) => {}
                        Err(err) => {
                            error!("{} err: <- {err:?}", transport.name);
                            break;
                        }
                    };
                }
                Err(err) => {
                    if !matches!(err, Error::StreamClosed) {
                        error!("Exiting {} after unexpected error: {err:?}", transport.name);
                    }

                    transport.close_pending().await;

                    // Hack: inject a terminated notification so we trigger code that needs to happen after exit
                    let notification =
                        ServerMessage::Call(jsonrpc::Call::Notification(jsonrpc::Notification {
                            jsonrpc: None,
                            method: lsp::notification::Exit::METHOD.to_string(),
                            params: jsonrpc::Params::None,
                        }));
                    match transport
                        .process_server_message(&client_tx, notification, &transport.name)
                        .await
                    {
                        Ok(_) => {}
                        Err(err) => {
                            error!("err: <- {:?}", err);
                        }
                    }
                    break;
                }
            }
        }
    }

    async fn err(transport: Arc<Self>, mut server_stderr: BufReader<ChildStderr>) {
        let mut recv_buffer = String::new();
        loop {
            match Self::recv_server_error(&mut server_stderr, &mut recv_buffer, &transport.name)
                .await
            {
                Ok(_) => {}
                Err(err) => {
                    error!("{} err: <- {err:?}", transport.name);
                    break;
                }
            }
        }
    }

    async fn send(
        transport: Arc<Self>,
        mut server_stdin: BufWriter<ChildStdin>,
        client_tx: UnboundedSender<(LanguageServerId, jsonrpc::Call)>,
        mut client_rx: UnboundedReceiver<Payload>,
        mut inject_rx: UnboundedReceiver<Payload>,
        initialize_notify: Arc<Notify>,
    ) {
        let mut pending_messages: Vec<Payload> = Vec::new();
        let mut is_pending = true;

        // Pin outside the loop to avoid cancellation-safety issue:
        // recreating `notified()` inside `select!` can lose the permit.
        let notified = initialize_notify.notified();
        tokio::pin!(notified);

        // Determine if a message is allowed to be sent early
        fn is_initialize(payload: &Payload) -> bool {
            use lsp::{
                notification::Initialized,
                request::{Initialize, Request},
            };
            match payload {
                Payload::Request {
                    value: jsonrpc::MethodCall { method, .. },
                    ..
                } if method == Initialize::METHOD => true,
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
            matches!(payload, Payload::Request { value: jsonrpc::MethodCall { method, .. }, .. } if method == Shutdown::METHOD)
        }

        fn is_exit(payload: &Payload) -> bool {
            use lsp::notification::{Exit, Notification};
            matches!(payload, Payload::Notification(jsonrpc::Notification { method, .. }) if method == Exit::METHOD)
        }

        // TODO: events that use capabilities need to do the right thing

        loop {
            tokio::select! {
                biased;
                _ = &mut notified, if is_pending => {
                    // server successfully initialized
                    is_pending = false;

                    // Hack: inject an initialized notification so we trigger code that needs to happen after init
                    let notification = ServerMessage::Call(jsonrpc::Call::Notification(jsonrpc::Notification {
                        jsonrpc: None,

                        method: lsp::notification::Initialized::METHOD.to_string(),
                        params: jsonrpc::Params::None,
                    }));
                    let language_server_name = &transport.name;
                    match transport.process_server_message(&client_tx, notification, language_server_name).await {
                        Ok(_) => {}
                        Err(err) => {
                            error!("{language_server_name} err: <- {err:?}");
                        }
                    }

                    // drain the pending queue and send payloads to server
                    for msg in pending_messages.drain(..) {
                        log::info!("Draining pending message {:?}", msg);
                        match transport.send_payload_to_server(&mut server_stdin, msg).await {
                            Ok(_) => {}
                            Err(err) => {
                                error!("{language_server_name} err: <- {err:?}");
                            }
                        }
                    }
                }
                msg = client_rx.recv() => {
                    if let Some(msg) = msg {
                        if is_pending && is_shutdown(&msg) {
                            log::info!("Language server not initialized, shutting down");
                            break;
                        } else if is_pending && !is_initialize(&msg) {
                            // ignore notifications
                            if let Payload::Notification(_) = msg {
                                continue;
                            }

                            log::info!("Language server not initialized, delaying request");
                            pending_messages.push(msg);
                        } else {
                            let is_shutdown_msg = is_shutdown(&msg);
                            let is_exit_msg = is_exit(&msg);
                            // Set the flag *before* flushing to stdin so that the recv task
                            // cannot observe an unanswered server request in the window between
                            // the kernel delivering the bytes to the server and this store.
                            if is_shutdown_msg {
                                transport
                                    .shutdown_requested
                                    .store(true, Ordering::Release);
                            }
                            match transport.send_payload_to_server(&mut server_stdin, msg).await {
                                Ok(_) => {
                                    // `exit` is the last thing a shutting-down client sends;
                                    // signal that it has reached the server's stdin.
                                    if is_exit_msg {
                                        transport.shutdown_flushed.notify_one();
                                    }
                                }
                                Err(err) => {
                                    error!("{} err: <- {err:?}", transport.name);
                                }
                            }
                        }
                    } else {
                        // channel closed
                        break;
                    }
                }
                msg = inject_rx.recv() => {
                    if let Some(msg) = msg {
                        match transport.send_payload_to_server(&mut server_stdin, msg).await {
                            Ok(_) => {}
                            Err(err) => {
                                error!("{} inject err: <- {err:?}", transport.name);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod cancellable_tests {
    use super::*;
    use slotmap::Key;
    use tokio::sync::{mpsc::channel, Semaphore};

    #[tokio::test]
    async fn hung_responses_hold_admission_until_cancel_and_late_reply_is_discarded() {
        let (inject_tx, _) = unbounded_channel();
        let transport = Transport {
            id: LanguageServerId::null(),
            name: "fixture".into(),
            pending_requests: Mutex::new(HashMap::default()),
            shutdown_requested: AtomicBool::new(false),
            inject_tx,
            shutdown_flushed: Arc::new(Notify::new()),
        };
        // The bounded pipe accepts outgoing frames but deliberately never replies.
        let (outgoing, mut listener) = tokio::io::duplex(64 * 1024);
        let mut server = BufWriter::new(outgoing);
        let permits = Arc::new(Semaphore::new(64));
        let mut receivers = Vec::new();
        for number in 0..64 {
            let (chan, receive) = channel(1);
            receivers.push(receive);
            transport
                .send_payload_to_server(
                    &mut server,
                    Payload::Request {
                        chan,
                        value: jsonrpc::MethodCall {
                            jsonrpc: Some(jsonrpc::Version::V2),
                            id: jsonrpc::Id::Num(number),
                            method: "textDocument/hover".into(),
                            params: jsonrpc::Params::None,
                        },
                        permit: Some(permits.clone().try_acquire_owned().unwrap()),
                    },
                )
                .await
                .unwrap();
        }
        assert_eq!(transport.pending_requests.lock().await.len(), 64);
        assert!(permits.clone().try_acquire_owned().is_err());
        for number in 0..64 {
            transport
                .send_payload_to_server(&mut server, Payload::Cancel(jsonrpc::Id::Num(number)))
                .await
                .unwrap();
        }
        assert!(transport.pending_requests.lock().await.is_empty());
        assert_eq!(permits.available_permits(), 64);
        assert!(receivers
            .iter_mut()
            .all(|receive| receive.try_recv().is_err()));
        transport
            .process_request_response(
                jsonrpc::Output::Success(jsonrpc::Success {
                    jsonrpc: Some(jsonrpc::Version::V2),
                    id: jsonrpc::Id::Num(1),
                    result: Value::Null,
                }),
                "fixture",
            )
            .await
            .unwrap();
        assert_eq!(permits.available_permits(), 64);
        assert!(transport.pending_requests.lock().await.is_empty());
        let (chan, mut receive) = channel(1);
        transport
            .send_payload_to_server(
                &mut server,
                Payload::Request {
                    chan,
                    value: jsonrpc::MethodCall {
                        jsonrpc: Some(jsonrpc::Version::V2),
                        id: jsonrpc::Id::Num(65),
                        method: "textDocument/hover".into(),
                        params: jsonrpc::Params::None,
                    },
                    permit: Some(permits.clone().try_acquire_owned().unwrap()),
                },
            )
            .await
            .unwrap();
        assert_eq!(permits.available_permits(), 63);
        transport.close_pending().await;
        assert!(matches!(
            receive.recv().await,
            Some(Err(Error::StreamClosed))
        ));
        assert_eq!(permits.available_permits(), 64);
        drop(server);
        let mut frames = String::new();
        listener.read_to_string(&mut frames).await.unwrap();
        assert_eq!(frames.matches("\"method\":\"$/cancelRequest\"").count(), 64);
    }
}
