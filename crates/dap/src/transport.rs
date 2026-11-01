use crate::{DebugAdapterId, Error, RequestError, Result};
use anyhow::Context;
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{Arc, Mutex as SequenceMutex},
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{
        mpsc::{unbounded_channel, Sender, UnboundedReceiver, UnboundedSender},
        oneshot, Mutex,
    },
};

#[derive(Debug, Clone, Deserialize, Serialize)]
/// DAP request envelope sent in either direction.
pub struct Request {
    /// Local response channel; skipped during serialization.
    #[serde(skip)]
    pub back_ch: Option<Sender<Result<Response>>>,
    pub seq: u64,
    pub command: String,
    pub arguments: Option<Value>,
}

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
/// DAP response envelope.
pub struct Response {
    /// Some adapters omit incoming sequence numbers; outgoing messages always have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub request_seq: u64,
    pub success: bool,
    pub command: String,
    pub message: Option<String>,
    pub body: Option<Value>,
}

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
/// DAP event envelope.
pub struct Event {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub event: String,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
/// Message exchanged between a [`Transport`] and its client-facing owner.
pub enum Payload {
    // type = "event"
    Event(Event),
    // type = "response"
    Response(Response),
    // type = "request"
    Request(Request),
}

#[derive(Debug)]
enum Outgoing {
    Message(Payload),
    Forget(u64),
}

/// Messages and lifecycle notifications for the owner of an adapter session.
#[derive(Debug)]
pub enum ClientEvent {
    Message(Payload),
    /// No further messages or requests can be exchanged on this transport.
    Closed {
        error: Option<String>,
    },
}

/// Owns the transport tasks. Dropping it closes both directions, even when request
/// senders remain alive. Request senders cannot extend a session's lifetime.
#[derive(Debug)]
pub struct Transport {
    _shutdown: oneshot::Sender<()>,
}

impl Transport {
    /// Start one cancellable task owning adapter input, output, and stderr.
    pub fn start(
        server_stdout: Box<dyn AsyncBufRead + Unpin + Send>,
        server_stdin: Box<dyn AsyncWrite + Unpin + Send>,
        server_stderr: Option<Box<dyn AsyncBufRead + Unpin + Send>>,
        id: DebugAdapterId,
    ) -> (
        Transport,
        UnboundedReceiver<(DebugAdapterId, ClientEvent)>,
        TransportSender,
    ) {
        let (client_tx, rx) = unbounded_channel();
        let (tx, client_rx) = unbounded_channel();
        let (shutdown, cancel) = oneshot::channel();
        tokio::spawn(async move {
            let state = TransportState {
                id,
                pending_requests: Mutex::new(HashMap::new()),
            };
            let result = tokio::select! {
                biased;
                _ = cancel => Ok(()),
                _ = client_tx.closed() => Ok(()),
                result = state.recv(server_stdout, &client_tx) => result,
                result = state.send(server_stdin, client_rx) => result,
                _ = async {
                    if let Some(stderr) = server_stderr { TransportState::err(stderr).await; }
                    // Stderr can close independently of the protocol connection.
                    std::future::pending::<()>().await;
                } => unreachable!(),
            };
            // Both IO futures have been dropped. New requests are rejected and all
            // outstanding waits complete before the owner sees the closure event.
            for (_, sender) in state.pending_requests.lock().await.drain() {
                let _ = sender.try_send(Err(Error::StreamClosed));
            }
            let error = match result {
                Ok(()) | Err(Error::StreamClosed) => None,
                Err(error) => Some(error.to_string()),
            };
            let _ = client_tx.send((id, ClientEvent::Closed { error }));
        });
        (
            Transport {
                _shutdown: shutdown,
            },
            rx,
            TransportSender {
                tx,
                sequence: Arc::new(SequenceMutex::new(1)),
            },
        )
    }
}

/// Sends protocol messages while keeping local request cleanup off the wire.
#[derive(Clone, Debug)]
pub struct TransportSender {
    tx: UnboundedSender<Outgoing>,
    sequence: Arc<SequenceMutex<u64>>,
}

impl TransportSender {
    pub fn send(&self, payload: Payload) -> Result<()> {
        self.send_message(payload).map(|_| ())
    }

    pub(crate) fn send_request(
        &self,
        command: &str,
        arguments: Option<Value>,
        back_ch: Sender<Result<Response>>,
    ) -> Result<u64> {
        self.send_message(Payload::Request(Request {
            back_ch: Some(back_ch),
            seq: 0,
            command: command.to_owned(),
            arguments,
        }))
    }

    fn send_message(&self, mut payload: Payload) -> Result<u64> {
        // Allocate and enqueue together so cloned senders preserve wire order.
        let mut next = self.sequence.lock().unwrap();
        let sequence = *next;
        match &mut payload {
            Payload::Request(request) => request.seq = sequence,
            Payload::Response(response) => response.seq = Some(sequence),
            Payload::Event(event) => event.seq = Some(sequence),
        }
        self.tx
            .send(Outgoing::Message(payload))
            .map_err(|_| Error::StreamClosed)?;
        *next += 1;
        Ok(sequence)
    }

    pub(crate) fn forget(&self, sequence: u64) {
        let _ = self.tx.send(Outgoing::Forget(sequence));
    }
}

struct TransportState {
    id: DebugAdapterId,
    pending_requests: Mutex<HashMap<u64, Sender<Result<Response>>>>,
}

impl TransportState {
    async fn recv_server_message(
        id: DebugAdapterId,
        reader: &mut Box<dyn AsyncBufRead + Unpin + Send>,
        buffer: &mut String,
        content: &mut Vec<u8>,
    ) -> Result<Payload> {
        let mut content_length = None;
        loop {
            buffer.clear();
            if reader.read_line(buffer).await? == 0 {
                return Err(Error::StreamClosed);
            };

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

        info!("[{}] <- DAP {}", id, msg);

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
    ) -> Result<()> {
        buffer.clear();
        if err.read_line(buffer).await? == 0 {
            return Err(Error::StreamClosed);
        };
        error!("err <- {}", buffer);

        Ok(())
    }

    async fn send_payload_to_server(
        &self,
        server_stdin: &mut Box<dyn AsyncWrite + Unpin + Send>,
        mut payload: Payload,
        json: &mut Vec<u8>,
    ) -> Result<()> {
        if let Payload::Request(request) = &mut payload
            && let Some(back) = request.back_ch.take()
        {
            self.pending_requests.lock().await.insert(request.seq, back);
        }
        json.clear();
        sonic_rs::to_writer(&mut *json, &payload)?;
        self.send_bytes_to_server(server_stdin, json).await?;
        // Reuse ordinary messages without retaining an unusually large document.
        if json.capacity() > 4 * 1024 * 1024 {
            *json = Vec::new();
        }
        Ok(())
    }

    async fn send_bytes_to_server(
        &self,
        server_stdin: &mut Box<dyn AsyncWrite + Unpin + Send>,
        request: &[u8],
    ) -> Result<()> {
        info!(
            "[{}] -> DAP {}",
            self.id,
            std::str::from_utf8(request).expect("JSON is valid UTF-8")
        );

        // send the headers
        server_stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", request.len()).as_bytes())
            .await?;

        // send the body
        server_stdin.write_all(request).await?;

        server_stdin.flush().await?;

        Ok(())
    }

    fn process_response(&self, res: Response) -> Result<Response> {
        if res.success {
            info!(
                "[{}] <- DAP success in response to {}",
                self.id, res.request_seq
            );

            Ok(res)
        } else {
            error!(
                "[{}] <- DAP error {:?} ({:?}) for command #{} {}",
                self.id, res.message, res.body, res.request_seq, res.command
            );

            Err(RequestError {
                command: res.command,
                request_seq: res.request_seq,
                message: res.message,
                body: res.body,
            }
            .into())
        }
    }

    async fn process_server_message(
        &self,
        client_tx: &UnboundedSender<(DebugAdapterId, ClientEvent)>,
        msg: Payload,
    ) -> Result<()> {
        match msg {
            Payload::Response(res) => {
                let request_seq = res.request_seq;
                let tx = self.pending_requests.lock().await.remove(&request_seq);

                match tx {
                    Some(tx) if !tx.is_closed() => {
                        match tx.send(self.process_response(res)).await {
                            Ok(_) => (),
                            Err(_) => {
                                log::debug!(
                                    "Discarding response to canceled request #{request_seq}"
                                )
                            }
                        }
                    }
                    Some(_) | None => {
                        log::debug!(
                            "Discarding response to completed or canceled request #{}",
                            res.request_seq
                        );
                    }
                }

                Ok(())
            }
            Payload::Request(Request {
                ref command,
                ref seq,
                ..
            }) => {
                info!("[{}] <- DAP request {} #{}", self.id, command, seq);
                client_tx
                    .send((self.id, ClientEvent::Message(msg)))
                    .map_err(|_| Error::StreamClosed)
            }
            Payload::Event(ref event) => {
                info!("[{}] <- DAP event {:?}", self.id, event);
                client_tx
                    .send((self.id, ClientEvent::Message(msg)))
                    .map_err(|_| Error::StreamClosed)
            }
        }
    }

    async fn recv(
        &self,
        mut server_stdout: Box<dyn AsyncBufRead + Unpin + Send>,
        client_tx: &UnboundedSender<(DebugAdapterId, ClientEvent)>,
    ) -> Result<()> {
        let mut recv_buffer = String::new();
        let mut content_buffer = Vec::new();
        loop {
            let message = Self::recv_server_message(
                self.id,
                &mut server_stdout,
                &mut recv_buffer,
                &mut content_buffer,
            )
            .await?;
            self.process_server_message(client_tx, message).await?;
        }
    }

    async fn send(
        &self,
        mut server_stdin: Box<dyn AsyncWrite + Unpin + Send>,
        mut client_rx: UnboundedReceiver<Outgoing>,
    ) -> Result<()> {
        let mut json = Vec::new();
        while let Some(command) = client_rx.recv().await {
            match command {
                Outgoing::Message(payload) => {
                    self.send_payload_to_server(&mut server_stdin, payload, &mut json)
                        .await?
                }
                Outgoing::Forget(sequence) => {
                    self.pending_requests.lock().await.remove(&sequence);
                }
            }
        }
        Ok(())
    }

    async fn err(mut server_stderr: Box<dyn AsyncBufRead + Unpin + Send>) {
        let mut recv_buffer = String::new();
        loop {
            match Self::recv_server_error(&mut server_stderr, &mut recv_buffer).await {
                Ok(_) => {}
                Err(err) => {
                    if !matches!(err, Error::StreamClosed) {
                        log::debug!("Adapter stderr closed: {err}");
                    }
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    struct BrokenWriter;
    impl AsyncWrite for BrokenWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn outgoing_buffer_reuses_small_capacity_and_releases_large_capacity() {
        let state = TransportState {
            id: DebugAdapterId::default(),
            pending_requests: Mutex::new(HashMap::new()),
        };
        let mut writer: Box<dyn AsyncWrite + Unpin + Send> = Box::new(tokio::io::sink());
        let mut json = Vec::new();
        for (size, oversized) in [(4096, false), (1, false), (5 * 1024 * 1024, true)] {
            state
                .send_payload_to_server(
                    &mut writer,
                    Payload::Event(Event {
                        seq: Some(1),
                        event: "output".into(),
                        body: Some(serde_json::json!({"text":"x".repeat(size)})),
                    }),
                    &mut json,
                )
                .await
                .unwrap();
            if oversized {
                assert_eq!(json.capacity(), 0);
            } else {
                assert!(json.capacity() >= 4096);
            }
        }
    }

    #[tokio::test]
    async fn write_failure_closes_the_transport_and_completes_the_request() {
        let (read, _adapter) = tokio::io::duplex(4096);
        let (_owner, mut incoming, sender) = Transport::start(
            Box::new(tokio::io::BufReader::new(read)),
            Box::new(BrokenWriter),
            None,
            DebugAdapterId::default(),
        );
        let (reply, mut response) = tokio::sync::mpsc::channel(1);
        sender
            .send(Payload::Request(Request {
                back_ch: Some(reply),
                seq: 1,
                command: "threads".into(),
                arguments: None,
            }))
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), incoming.recv())
                .await
                .unwrap(),
            Some((_, ClientEvent::Closed { error: Some(_) }))
        ));
        assert!(matches!(
            response.recv().await,
            Some(Err(Error::StreamClosed))
        ));
        assert!(incoming.recv().await.is_none());
    }
}
