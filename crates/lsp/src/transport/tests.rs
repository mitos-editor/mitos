use super::*;
use serde_json::Value;
use slotmap::Key;
use std::time::Duration;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncWrite, DuplexStream, WriteHalf},
    task::JoinHandle,
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

struct Fixture {
    transport: Arc<Transport>,
    sender: UnboundedSender<Payload>,
    server: BufReader<DuplexStream>,
    initialized: Arc<Notify>,
    messages: UnboundedReceiver<(LanguageServerId, jsonrpc::Call)>,
    writer: JoinHandle<()>,
    reader: JoinHandle<()>,
}

impl Fixture {
    fn new(capacity: usize, initialized: bool) -> Self {
        Self::with_writer(capacity, initialized, |writer| writer)
    }

    fn with_writer<W: AsyncWrite + Unpin + Send + 'static>(
        capacity: usize,
        initialized: bool,
        writer: impl FnOnce(WriteHalf<DuplexStream>) -> W,
    ) -> Self {
        let (sender, outgoing) = unbounded_channel();
        let (inject_tx, inject_rx) = unbounded_channel();
        let (client_tx, messages) = unbounded_channel();
        let transport = Arc::new(Transport {
            id: LanguageServerId::null(),
            name: "fixture".into(),
            pending_requests: Arc::new(PendingRequests::default()),
            shutdown_requested: AtomicBool::new(false),
            inject_tx,
            shutdown_flushed: Arc::new(Notify::new()),
        });
        let initialize_notify = Arc::new(Notify::new());
        if initialized {
            initialize_notify.notify_one();
        }
        let (client, server) = tokio::io::duplex(capacity);
        let (read, write) = tokio::io::split(client);
        let reader = tokio::spawn(Transport::recv(
            transport.clone(),
            BufReader::new(read),
            client_tx.clone(),
        ));
        let writer = tokio::spawn(Transport::send(
            transport.clone(),
            BufWriter::new(writer(write)),
            client_tx,
            outgoing,
            inject_rx,
            initialize_notify.clone(),
        ));
        Self {
            transport,
            sender,
            server: BufReader::new(server),
            initialized: initialize_notify,
            messages,
            writer,
            reader,
        }
    }

    fn request(
        &self,
        id: u64,
        timeout_secs: u64,
    ) -> impl std::future::Future<Output = Result<Option<lsp::Hover>>> + use<> {
        self.transport
            .pending_requests
            .request::<lsp::request::HoverRequest>(
                &self.sender,
                jsonrpc::Id::Num(id),
                &lsp::HoverParams {
                    text_document_position_params: lsp::TextDocumentPositionParams {
                        text_document: lsp::TextDocumentIdentifier::new(
                            lsp::Url::parse("file:///test.rs").unwrap(),
                        ),
                        position: lsp::Position::new(0, 0),
                    },
                    work_done_progress_params: Default::default(),
                },
                timeout_secs,
            )
    }

    async fn next(&mut self) -> Value {
        let message = tokio::time::timeout(
            Duration::from_secs(5),
            Transport::recv_server_message(
                &mut self.server,
                &mut String::new(),
                &mut Vec::new(),
                "fixture",
            ),
        )
        .await
        .unwrap()
        .unwrap();
        serde_json::to_value(message).unwrap()
    }

    async fn respond(&mut self, response: Value) {
        let body = response.to_string();
        self.server
            .get_mut()
            .write_all(format!("Content-Length: {}\r\n\r\n{}", body.len(), body).as_bytes())
            .await
            .unwrap();
    }

    fn pending(&self) -> usize {
        self.transport.pending_requests.len()
    }

    async fn finish(mut self) -> Vec<u8> {
        if !self.transport.shutdown_requested.load(Ordering::Acquire) {
            // Drain the outgoing queue before closing; otherwise closure could
            // hide an unexpected cancellation frame behind an aborted write.
            self.sender
                .send(Payload::Notification(jsonrpc::Notification {
                    jsonrpc: Some(jsonrpc::Version::V2),
                    method: "fixture/barrier".into(),
                    params: jsonrpc::Params::None,
                }))
                .unwrap();
            assert_eq!(self.next().await["method"], "fixture/barrier");
        }
        self.server.get_mut().shutdown().await.unwrap();
        self.reader.await.unwrap();
        self.writer.await.unwrap();
        let mut remaining = Vec::new();
        self.server.read_to_end(&mut remaining).await.unwrap();
        remaining
    }
}

#[tokio::test]
async fn dropping_an_unpolled_request_skips_transmission() {
    let mut fixture = Fixture::new(4096, true);
    let abandoned = fixture.request(1, 20);
    drop(abandoned);
    assert_eq!(fixture.pending(), 0);
    let live = fixture.request(2, 20);
    assert_eq!(fixture.next().await["id"], 2);
    fixture
        .respond(serde_json::json!({"jsonrpc":"2.0", "id":2, "result":null}))
        .await;
    assert!(live.await.unwrap().is_none());
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test]
async fn dropping_an_active_request_cancels_once_and_ignores_late_responses() {
    let mut fixture = Fixture::new(4096, true);
    let request = fixture.request(1, 20);
    assert_eq!(fixture.next().await["id"], 1);
    drop(request);
    assert_eq!(fixture.pending(), 0);
    let cancel = fixture.next().await;
    assert_eq!(cancel["method"], "$/cancelRequest");
    assert_eq!(cancel["params"]["id"], 1);
    fixture
        .respond(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "error": {"code": -32800, "message": "cancelled"},
        }))
        .await;
    let next = fixture.request(2, 20);
    assert_eq!(fixture.next().await["id"], 2);
    fixture
        .respond(serde_json::json!({"jsonrpc":"2.0", "id":2, "result":null}))
        .await;
    assert!(next.await.unwrap().is_none());
    assert_eq!(fixture.pending(), 0);
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn requests_use_the_configured_deadline_and_cancel_on_timeout() {
    let mut fixture = Fixture::new(4096, true);
    let request = tokio::spawn(fixture.request(1, 10));
    assert_eq!(fixture.next().await["id"], 1);
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(!request.is_finished());
    assert_eq!(fixture.pending(), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(
        request.await.unwrap(),
        Err(Error::Timeout(jsonrpc::Id::Num(1)))
    ));
    assert_eq!(fixture.pending(), 0);
    assert_eq!(fixture.next().await["method"], "$/cancelRequest");
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn time_waiting_to_poll_counts_toward_the_request_deadline() {
    let mut fixture = Fixture::new(4096, true);
    let request = fixture.request(1, 10);
    assert_eq!(fixture.next().await["id"], 1);
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(matches!(request.await, Err(Error::Timeout(_))));
    assert_eq!(fixture.next().await["method"], "$/cancelRequest");
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test]
async fn completed_requests_do_not_send_cancellation() {
    let mut fixture = Fixture::new(4096, true);
    let success = fixture.request(1, 20);
    assert_eq!(fixture.next().await["id"], 1);
    fixture
        .respond(serde_json::json!({"jsonrpc":"2.0", "id":1, "result":null}))
        .await;
    assert!(success.await.unwrap().is_none());
    let failure = fixture.request(2, 20);
    assert_eq!(fixture.next().await["id"], 2);
    fixture
        .respond(serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "error": {"code": -32603, "message": "failure"},
        }))
        .await;
    assert!(matches!(failure.await, Err(Error::Rpc(_))));
    let invalid = fixture.request(3, 20);
    assert_eq!(fixture.next().await["id"], 3);
    fixture
        .respond(serde_json::json!({"jsonrpc":"2.0", "id":3, "result":42}))
        .await;
    assert!(matches!(invalid.await, Err(Error::Parse(_))));
    assert_eq!(fixture.pending(), 0);
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test]
async fn cancellation_releases_local_tracking_while_a_write_is_stalled() {
    let mut fixture = Fixture::new(1, true);
    let request = fixture.request(1, 20);
    // Reading one byte proves the writer has started and is blocked mid-frame.
    assert_eq!(fixture.server.read_u8().await.unwrap(), b'C');
    drop(request);
    assert_eq!(fixture.pending(), 0);
    // Let the original frame finish before the cancellation notification.
    let mut first = String::from("C");
    fixture.server.read_line(&mut first).await.unwrap();
    assert!(first.starts_with("Content-Length:"));
    let length: usize = first.trim().split_once(": ").unwrap().1.parse().unwrap();
    let mut rest = vec![0; length + 2];
    fixture.server.read_exact(&mut rest).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&rest[2..]).unwrap()["id"],
        1
    );
    assert_eq!(fixture.next().await["method"], "$/cancelRequest");
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test]
async fn stream_closure_interrupts_a_stalled_write_and_closes_queued_requests() {
    let mut fixture = Fixture::new(1, true);
    let active = fixture.request(1, 20);
    let queued = fixture.request(2, 20);
    fixture.server.read_u8().await.unwrap();
    fixture.server.get_mut().shutdown().await.unwrap();
    assert!(matches!(active.await, Err(Error::StreamClosed)));
    assert!(matches!(queued.await, Err(Error::StreamClosed)));
    assert!(matches!(
        fixture.request(3, 20).await,
        Err(Error::StreamClosed)
    ));
    assert_eq!(fixture.pending(), 0);
    fixture.reader.await.unwrap();
    fixture.writer.await.unwrap();
    let exits = std::iter::from_fn(|| fixture.messages.try_recv().ok())
        .filter(|(_, call)| matches!(call, jsonrpc::Call::Notification(notification) if notification.method == "exit"))
        .count();
    assert_eq!(exits, 1);
}

#[tokio::test(start_paused = true)]
async fn write_failure_notifies_the_editor_and_stops_the_waiting_reader() {
    let mut fixture = Fixture::with_writer(4096, false, |_| BrokenWriter);
    let active = fixture
        .transport
        .pending_requests
        .request::<lsp::request::Initialize>(
            &fixture.sender,
            jsonrpc::Id::Num(1),
            &lsp::InitializeParams::default(),
            20,
        );
    assert!(matches!(active.await, Err(Error::StreamClosed)));
    fixture.writer.await.unwrap();
    let (_, exit) = tokio::time::timeout(Duration::from_secs(5), fixture.messages.recv())
        .await
        .expect("write failure must notify the editor even while stdout stays open")
        .expect("editor channel closed without an exit notification");
    assert!(
        matches!(exit, jsonrpc::Call::Notification(notification) if notification.method == "exit")
    );
    tokio::time::timeout(Duration::from_secs(5), fixture.reader)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        fixture
            .transport
            .pending_requests
            .request::<lsp::request::Shutdown>(&fixture.sender, jsonrpc::Id::Num(2), &(), 20,)
            .await,
        Err(Error::StreamClosed)
    ));
    assert!(
        fixture.messages.try_recv().is_err(),
        "termination must notify the editor only once"
    );
}

#[tokio::test]
async fn a_failed_enqueue_leaves_no_pending_response_handler() {
    let (sender, receiver) = unbounded_channel();
    drop(receiver);
    let pending = Arc::new(PendingRequests::default());
    let request = pending.request::<lsp::request::Shutdown>(&sender, jsonrpc::Id::Num(1), &(), 20);
    assert_eq!(pending.len(), 0);
    assert!(matches!(request.await, Err(Error::StreamClosed)));
}

#[tokio::test]
async fn pending_request_futures_do_not_keep_the_outgoing_channel_alive() {
    let mut fixture = Fixture::new(4096, true);
    let request = fixture.request(1, 20);
    assert_eq!(fixture.next().await["id"], 1);
    drop(fixture.sender);
    assert!(matches!(request.await, Err(Error::StreamClosed)));
    fixture.writer.await.unwrap();
    fixture.server.get_mut().shutdown().await.unwrap();
    fixture.reader.await.unwrap();
}

#[tokio::test]
async fn requests_cancelled_during_initialization_are_never_sent() {
    let mut fixture = Fixture::new(4096, false);
    let abandoned = fixture.request(1, 20);
    tokio::task::yield_now().await;
    drop(abandoned);
    let live = fixture.request(2, 20);
    fixture.initialized.notify_one();
    assert_eq!(fixture.next().await["id"], 2);
    fixture
        .respond(serde_json::json!({"jsonrpc":"2.0", "id":2, "result":null}))
        .await;
    assert!(live.await.unwrap().is_none());
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test]
async fn dropping_the_shutdown_receiver_preserves_fire_and_forget_ordering() {
    let mut fixture = Fixture::new(4096, true);
    fixture
        .transport
        .pending_requests
        .send_fire_and_forget(
            &fixture.sender,
            jsonrpc::MethodCall {
                jsonrpc: Some(jsonrpc::Version::V2),
                id: jsonrpc::Id::Num(1),
                method: "shutdown".into(),
                params: jsonrpc::Params::None,
            },
        )
        .unwrap();
    fixture
        .sender
        .send(Payload::Notification(jsonrpc::Notification {
            jsonrpc: Some(jsonrpc::Version::V2),
            method: "exit".into(),
            params: jsonrpc::Params::None,
        }))
        .unwrap();
    assert_eq!(fixture.next().await["method"], "shutdown");
    assert_eq!(fixture.next().await["method"], "exit");
    assert!(fixture.finish().await.is_empty());
}
