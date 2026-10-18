use crate::{DebugAdapterId, Error, Result};
use anyhow::Context;
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::{collections::HashMap, fmt::Debug};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{
        mpsc::{unbounded_channel, Sender, UnboundedReceiver, UnboundedSender},
        Mutex,
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
    // seq is omitted as unused and is not sent by some implementations
    pub request_seq: u64,
    pub success: bool,
    pub command: String,
    pub message: Option<String>,
    pub body: Option<Value>,
}

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
/// DAP event envelope.
pub struct Event {
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

/// Sends protocol messages while keeping local request cleanup off the wire.
#[derive(Clone, Debug)]
pub struct TransportSender(UnboundedSender<Outgoing>);

impl TransportSender {
    pub fn send(&self, payload: Payload) -> Result<()> {
        self.0
            .send(Outgoing::Message(payload))
            .map_err(|error| Error::Other(error.into()))
    }

    pub(crate) fn forget(&self, sequence: u64) {
        let _ = self.0.send(Outgoing::Forget(sequence));
    }
}

#[derive(Debug)]
/// Content-Length framed DAP transport over asynchronous byte streams.
///
/// Responses to outgoing requests are delivered to their private response
/// channels. Events and reverse requests are forwarded
/// on the receiver returned by [`Self::start`]. Late responses to canceled
/// requests are discarded.
pub struct Transport {
    #[allow(unused)]
    id: DebugAdapterId,
    pending_requests: Mutex<HashMap<u64, Sender<Result<Response>>>>,
}

impl Transport {
    /// Starts background receive, send, and optional stderr-forwarding tasks.
    ///
    /// The returned tuple is `(incoming, outgoing)`. Dropping the outgoing
    /// sender eventually stops the send task; dropping the incoming receiver
    /// means forwarded adapter messages can no longer be consumed.
    pub fn start(
        server_stdout: Box<dyn AsyncBufRead + Unpin + Send>,
        server_stdin: Box<dyn AsyncWrite + Unpin + Send>,
        server_stderr: Option<Box<dyn AsyncBufRead + Unpin + Send>>,
        id: DebugAdapterId,
    ) -> (UnboundedReceiver<Payload>, TransportSender) {
        let (client_tx, rx) = unbounded_channel();
        let (tx, client_rx) = unbounded_channel();

        let transport = Self {
            id,
            pending_requests: Mutex::new(HashMap::default()),
        };

        let transport = Arc::new(transport);

        tokio::spawn(Self::recv(id, transport.clone(), server_stdout, client_tx));
        tokio::spawn(Self::send(transport, server_stdin, client_rx));
        if let Some(stderr) = server_stderr {
            tokio::spawn(Self::err(stderr));
        }

        (rx, TransportSender(tx))
    }

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
    ) -> Result<()> {
        if let Payload::Request(request) = &mut payload
            && let Some(back) = request.back_ch.take()
        {
            self.pending_requests.lock().await.insert(request.seq, back);
        }
        let json = serde_json::to_string(&payload)?;
        self.send_string_to_server(server_stdin, json).await
    }

    async fn send_string_to_server(
        &self,
        server_stdin: &mut Box<dyn AsyncWrite + Unpin + Send>,
        request: String,
    ) -> Result<()> {
        info!("[{}] -> DAP {}", self.id, request);

        // send the headers
        server_stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", request.len()).as_bytes())
            .await?;

        // send the body
        server_stdin.write_all(request.as_bytes()).await?;

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

            Err(Error::Other(anyhow::format_err!("{:?}", res.body)))
        }
    }

    async fn process_server_message(
        &self,
        client_tx: &UnboundedSender<Payload>,
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
                client_tx.send(msg).map_err(|_| Error::StreamClosed)
            }
            Payload::Event(ref event) => {
                info!("[{}] <- DAP event {:?}", self.id, event);
                client_tx.send(msg).map_err(|_| Error::StreamClosed)
            }
        }
    }

    async fn recv(
        id: DebugAdapterId,
        transport: Arc<Self>,
        mut server_stdout: Box<dyn AsyncBufRead + Unpin + Send>,
        client_tx: UnboundedSender<Payload>,
    ) {
        let mut recv_buffer = String::new();
        let mut content_buffer = Vec::new();
        loop {
            match Self::recv_server_message(
                id,
                &mut server_stdout,
                &mut recv_buffer,
                &mut content_buffer,
            )
            .await
            {
                Ok(msg) => match transport.process_server_message(&client_tx, msg).await {
                    Ok(_) => (),
                    Err(err) => {
                        if !matches!(err, Error::StreamClosed) {
                            error!(" [{id}] err: <- {err:?}");
                        }
                        break;
                    }
                },
                Err(err) => {
                    if !matches!(err, Error::StreamClosed) {
                        error!("Exiting after unexpected error: {err:?}");
                    }

                    // Close any outstanding requests.
                    let pending_requests: Vec<_> = {
                        let mut pending_requests = transport.pending_requests.lock().await;
                        pending_requests.drain().collect()
                    };
                    for (id, tx) in pending_requests {
                        match tx.send(Err(Error::StreamClosed)).await {
                            Ok(_) => (),
                            Err(_) => {
                                error!("Could not close request on a closed channel (id={id})");
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    async fn send_inner(
        transport: Arc<Self>,
        mut server_stdin: Box<dyn AsyncWrite + Unpin + Send>,
        mut client_rx: UnboundedReceiver<Outgoing>,
    ) -> Result<()> {
        while let Some(command) = client_rx.recv().await {
            match command {
                Outgoing::Message(payload) => {
                    transport
                        .send_payload_to_server(&mut server_stdin, payload)
                        .await?
                }
                Outgoing::Forget(sequence) => {
                    transport.pending_requests.lock().await.remove(&sequence);
                }
            }
        }
        Ok(())
    }

    async fn send(
        transport: Arc<Self>,
        server_stdin: Box<dyn AsyncWrite + Unpin + Send>,
        client_rx: UnboundedReceiver<Outgoing>,
    ) {
        if let Err(err) = Self::send_inner(transport, server_stdin, client_rx).await {
            error!("err: <- {:?}", err);
        }
    }

    async fn err(mut server_stderr: Box<dyn AsyncBufRead + Unpin + Send>) {
        let mut recv_buffer = String::new();
        loop {
            match Self::recv_server_error(&mut server_stderr, &mut recv_buffer).await {
                Ok(_) => {}
                Err(err) => {
                    error!("err: <- {:?}", err);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn adapter_reads_end_when_the_client_receiver_closes() {
        for payload in [
            json!({"type":"event", "event":"initialized"}),
            json!({"type":"request", "seq":1, "command":"runInTerminal"}),
        ] {
            let id = DebugAdapterId::default();
            let transport = Arc::new(Transport {
                id,
                pending_requests: Mutex::new(HashMap::new()),
            });
            let (client_tx, client_rx) = unbounded_channel();
            drop(client_rx);
            let (read, mut write) = tokio::io::duplex(4096);
            let body = serde_json::to_vec(&payload).unwrap();
            write
                .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            write.write_all(&body).await.unwrap();
            drop(write);
            Transport::recv(
                id,
                transport,
                Box::new(tokio::io::BufReader::new(read)),
                client_tx,
            )
            .await;
        }
    }
}
