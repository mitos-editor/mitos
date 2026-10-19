mod tcp;

use crate::{
    requests::{DisconnectArguments, TerminateArguments},
    transport::{ClientEvent, Payload, Response, Transport, TransportSender},
    DebugAdapterId, Error, Result,
};
use dap_types::*;

use serde_json::Value;

use std::{future::Future, net::SocketAddr, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncBufRead, AsyncWrite, BufReader, BufWriter},
    net::TcpStream,
    process::{Child, Command},
    sync::mpsc::{channel, UnboundedReceiver},
};

#[derive(Debug)]
/// One debug-adapter transport and its typed protocol requests.
///
/// Construction returns a separate receiver for adapter events and reverse
/// requests. Callers must continue polling that receiver while awaiting typed
/// requests or an adapter can deadlock waiting for a reverse-request reply.
pub struct Client {
    id: DebugAdapterId,
    _process: Option<Child>,
    _transport: Transport,
    server_tx: TransportSender,
    /// The socket address of the debugger, if using TCP transport.
    pub socket: Option<SocketAddr>,
}

impl Client {
    /// Builds a client around already-open asynchronous streams.
    ///
    /// This is the common constructor for TCP, stdio, and test transports. If
    /// `process` is present, its handle remains owned by the client for the
    /// session lifetime; termination behavior is chosen by the constructor.
    pub fn streams(
        rx: Box<dyn AsyncBufRead + Unpin + Send>,
        tx: Box<dyn AsyncWrite + Unpin + Send>,
        err: Option<Box<dyn AsyncBufRead + Unpin + Send>>,
        id: DebugAdapterId,
        process: Option<Child>,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, ClientEvent)>)> {
        let (transport, incoming, server_tx) = Transport::start(rx, tx, err, id);

        let client = Self {
            id,
            _process: process,
            _transport: transport,
            server_tx,
            socket: None,
        };

        Ok((client, incoming))
    }

    /// Connects to an already-running TCP debug adapter.
    pub async fn tcp(
        addr: std::net::SocketAddr,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, ClientEvent)>)> {
        let stream = TcpStream::connect(addr).await?;
        let (rx, tx) = stream.into_split();
        let (mut client, incoming) =
            Self::streams(Box::new(BufReader::new(rx)), Box::new(tx), None, id, None)?;
        client.socket = Some(addr);
        Ok((client, incoming))
    }

    /// Spawns a debug adapter whose stdin/stdout carry DAP messages.
    pub fn stdio(
        cmd: &str,
        args: Vec<&str>,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, ClientEvent)>)> {
        // Resolve path to the binary
        let cmd = stdx::env::which(cmd)?;

        let process = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // make sure the process is reaped on drop
            .kill_on_drop(true)
            .spawn();

        let mut process = process?;

        // TODO: do we need bufreader/writer here? or do we use async wrappers on unblock?
        let writer = BufWriter::new(process.stdin.take().expect("Failed to open stdin"));
        let reader = BufReader::new(process.stdout.take().expect("Failed to open stdout"));
        let stderr = BufReader::new(process.stderr.take().expect("Failed to open stderr"));

        Self::streams(
            Box::new(reader),
            Box::new(writer),
            Some(Box::new(stderr)),
            id,
            Some(process),
        )
    }

    /// Spawns an adapter that listens on an ephemeral loopback TCP port.
    ///
    /// Every `{}` in `port_format` is replaced with the selected port. The
    /// resulting string is split on whitespace and appended to `args`. Startup
    /// retries connection refusals for up to ten seconds and reports early process
    /// exit immediately. Dropping startup kills the pending adapter process.
    pub async fn tcp_process(
        cmd: &str,
        args: Vec<&str>,
        port_format: &str,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, ClientEvent)>)> {
        let (stream, process, socket) = tcp::spawn(cmd, args, port_format).await?;
        let (rx, tx) = stream.into_split();
        let mut result = Self::streams(
            Box::new(BufReader::new(rx)),
            Box::new(tx),
            None,
            id,
            Some(process),
        );

        // Set the socket address for the client
        if let Ok((client, _)) = &mut result {
            client.socket = Some(socket);
        }

        result
    }

    /// Returns this client's registry identifier.
    pub fn id(&self) -> DebugAdapterId {
        self.id
    }

    /// An owned transport handle for requests that must outlive a borrow of editor state.
    pub fn request_sender(&self) -> RequestSender {
        RequestSender {
            server_tx: self.server_tx.clone(),
        }
    }

    /// Execute a typed request and return its JSON body.
    pub fn call<R: dap_types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>> + use<R>
    where
        R::Arguments: serde::Serialize,
    {
        self.request_sender().call::<R>(arguments)
    }

    pub async fn request<R: dap_types::Request>(&self, arguments: R::Arguments) -> Result<R::Result>
    where
        R::Arguments: serde::Serialize,
    {
        self.request_sender().request::<R>(arguments).await
    }

    /// Queues a reply to a reverse request initiated by the debug adapter.
    pub fn reply(
        &self,
        request_seq: u64,
        command: &str,
        result: core::result::Result<Value, Error>,
    ) -> Result<()> {
        let response = match result {
            Ok(result) => Response {
                seq: None,
                request_seq,
                command: command.to_owned(),
                success: true,
                message: None,
                body: Some(result),
            },
            Err(error) => Response {
                seq: None,
                request_seq,
                command: command.to_owned(),
                success: false,
                message: Some(format!("{error:#}")),
                body: None,
            },
        };
        self.server_tx.send(Payload::Response(response))
    }

    /// Return an owned initialization request. The owner installs its capabilities
    /// while continuing to serve incoming events and reverse requests.
    pub fn initialize(
        &self,
        adapter_id: String,
        supports_run_in_terminal: bool,
        supports_start_debugging: bool,
    ) -> impl Future<Output = Result<DebuggerCapabilities>> + use<> {
        let args = requests::InitializeArguments {
            client_id: Some("mitos".to_owned()),
            client_name: Some("mitos".to_owned()),
            adapter_id,
            locale: Some("en-us".to_owned()),
            lines_start_at_one: Some(true),
            columns_start_at_one: Some(true),
            path_format: Some("path".to_owned()),
            supports_variable_type: Some(true),
            supports_variable_paging: Some(true),
            supports_run_in_terminal_request: Some(supports_run_in_terminal),
            supports_start_debugging_request: Some(supports_start_debugging),
            supports_memory_references: Some(false),
            supports_progress_reporting: Some(true),
            supports_invalidated_event: Some(true),
        };

        self.request_sender().request::<requests::Initialize>(args)
    }

    /// Validate a standard acknowledgement before discarding its empty body.
    fn acknowledgement<R: dap_types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<()>> + use<R>
    where
        R::Arguments: serde::Serialize,
    {
        let response = self.request_sender().request::<R>(arguments);
        async move {
            response.await?;
            Ok(())
        }
    }

    pub fn disconnect(
        &self,
        args: Option<DisconnectArguments>,
    ) -> impl Future<Output = Result<()>> + use<> {
        self.acknowledgement::<requests::Disconnect>(args)
    }

    pub fn terminate(
        &self,
        args: Option<TerminateArguments>,
    ) -> impl Future<Output = Result<()>> + use<> {
        self.acknowledgement::<requests::Terminate>(args)
    }

    pub fn launch(&self, args: serde_json::Value) -> impl Future<Output = Result<()>> + use<> {
        self.acknowledgement::<requests::Launch>(args)
    }

    pub fn attach(&self, args: serde_json::Value) -> impl Future<Output = Result<()>> + use<> {
        self.acknowledgement::<requests::Attach>(args)
    }

    pub fn restart(&self, args: Value) -> impl Future<Output = Result<()>> + use<> {
        self.acknowledgement::<requests::Restart>(args)
    }

    pub async fn set_breakpoints(
        &self,
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<Option<Vec<Breakpoint>>> {
        let args = requests::SetBreakpointsArguments {
            source: Source {
                path: Some(file),
                name: None,
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            },
            breakpoints: Some(breakpoints),
            source_modified: Some(false),
        };

        let response = self.request::<requests::SetBreakpoints>(args).await?;

        Ok(response.breakpoints)
    }

    /// Sends configuration completion after the owner checks adapter support.
    pub async fn configuration_done(&self) -> Result<()> {
        self.request::<requests::ConfigurationDone>(Some(requests::ConfigurationDoneArguments {}))
            .await
            .map(|_| ())
    }

    pub fn continue_thread(
        &self,
        thread_id: ThreadId,
    ) -> impl Future<Output = Result<requests::ContinueResponse>> + use<> {
        let args = requests::ContinueArguments { thread_id };

        self.request_sender().request::<requests::Continue>(args)
    }

    pub async fn stack_trace(
        &self,
        thread_id: ThreadId,
    ) -> Result<(Vec<StackFrame>, Option<usize>)> {
        let args = requests::StackTraceArguments {
            thread_id,
            start_frame: None,
            levels: None,
            format: None,
        };

        let response = self.request::<requests::StackTrace>(args).await?;
        Ok((response.stack_frames, response.total_frames))
    }

    pub fn threads(&self) -> impl Future<Output = Result<requests::ThreadsResponse>> + use<> {
        self.request_sender()
            .request::<requests::Threads>(Some(requests::ThreadsArguments {}))
    }

    pub async fn scopes(&self, frame_id: usize) -> Result<Vec<Scope>> {
        let args = requests::ScopesArguments { frame_id };

        let response = self.request::<requests::Scopes>(args).await?;
        Ok(response.scopes)
    }

    pub async fn variables(&self, variables_reference: usize) -> Result<Vec<Variable>> {
        let args = requests::VariablesArguments {
            variables_reference,
            filter: None,
            start: None,
            count: None,
            format: None,
        };

        let response = self.request::<requests::Variables>(args).await?;
        Ok(response.variables)
    }

    pub fn step_in(&self, thread_id: ThreadId) -> impl Future<Output = Result<()>> + use<> {
        let args = requests::StepInArguments {
            thread_id,
            target_id: None,
            granularity: None,
        };

        self.acknowledgement::<requests::StepIn>(args)
    }

    pub fn step_out(&self, thread_id: ThreadId) -> impl Future<Output = Result<()>> + use<> {
        let args = requests::StepOutArguments {
            thread_id,
            granularity: None,
        };

        self.acknowledgement::<requests::StepOut>(args)
    }

    pub fn next(&self, thread_id: ThreadId) -> impl Future<Output = Result<()>> + use<> {
        let args = requests::NextArguments {
            thread_id,
            granularity: None,
        };

        self.acknowledgement::<requests::Next>(args)
    }

    pub fn pause(&self, thread_id: ThreadId) -> impl Future<Output = Result<()>> + use<> {
        let args = requests::PauseArguments { thread_id };

        self.acknowledgement::<requests::Pause>(args)
    }

    pub async fn eval(
        &self,
        expression: String,
        frame_id: Option<usize>,
    ) -> Result<requests::EvaluateResponse> {
        let args = requests::EvaluateArguments {
            expression,
            frame_id,
            context: None,
            format: None,
        };

        self.request::<requests::Evaluate>(args).await
    }

    pub fn set_exception_breakpoints(
        &self,
        filters: Vec<String>,
    ) -> impl Future<Output = Result<requests::SetExceptionBreakpointsResponse>> + use<> {
        let args = requests::SetExceptionBreakpointsArguments { filters };

        self.request_sender()
            .request::<requests::SetExceptionBreakpoints>(args)
    }
}

/// Cloneable request transport. It carries no selected thread, frame, or editor state.
#[derive(Clone, Debug)]
pub struct RequestSender {
    server_tx: TransportSender,
}

impl RequestSender {
    /// Executes a typed request and returns its untyped JSON body.
    ///
    /// Requests use monotonically increasing DAP sequence numbers and time out
    /// after 20 seconds. Prefer [`Self::request`] when the typed result is known.
    pub fn call<R: dap_types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>> + use<R>
    where
        R::Arguments: serde::Serialize,
    {
        let server_tx = self.server_tx.clone();

        async move {
            use std::time::Duration;
            use tokio::time::timeout;

            let arguments = Some(serde_json::to_value(arguments)?);

            let (callback_tx, mut callback_rx) = channel(1);

            let id = server_tx.send_request(R::COMMAND, arguments, callback_tx)?;

            // Dropping the wait releases transport bookkeeping, including on cancellation.
            let _pending = PendingRequest {
                sender: server_tx,
                sequence: id,
            };
            let response = timeout(Duration::from_secs(20), callback_rx.recv())
                .await
                .map_err(|_| Error::Timeout(id))? // return Timeout
                .ok_or(Error::StreamClosed)??;

            Ok(response.body.unwrap_or_default())
        }
    }

    /// Executes a typed request without retaining a borrow of the sender.
    pub fn request<R: dap_types::Request>(
        &self,
        params: R::Arguments,
    ) -> impl Future<Output = Result<R::Result>> + use<R>
    where
        R::Arguments: serde::Serialize,
    {
        let response = self.call::<R>(params);
        async move { Ok(serde_json::from_value(response.await?)?) }
    }
}

struct PendingRequest {
    sender: TransportSender,
    sequence: u64,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.sender.forget(self.sequence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    use tokio::time;

    fn connection() -> (
        Client,
        UnboundedReceiver<(DebugAdapterId, ClientEvent)>,
        tokio::io::DuplexStream,
    ) {
        let (client, adapter) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let (client, incoming) = Client::streams(
            Box::new(BufReader::new(read)),
            Box::new(write),
            None,
            DebugAdapterId::default(),
            None,
        )
        .unwrap();
        (client, incoming, adapter)
    }

    async fn read_message(adapter: &mut BufReader<tokio::io::DuplexStream>) -> Value {
        time::timeout(time::Duration::from_secs(5), async {
            let mut header = String::new();
            adapter.read_line(&mut header).await.unwrap();
            let length: usize = header
                .trim()
                .strip_prefix("Content-Length: ")
                .unwrap()
                .parse()
                .unwrap();
            header.clear();
            adapter.read_line(&mut header).await.unwrap();
            assert_eq!(header, "\r\n");
            let mut body = vec![0; length];
            adapter.read_exact(&mut body).await.unwrap();
            serde_json::from_slice(&body).unwrap()
        })
        .await
        .unwrap()
    }

    async fn reply_to(
        adapter: &mut BufReader<tokio::io::DuplexStream>,
        request: &Value,
        body: Value,
    ) {
        let response = json!({
            "type": "response", "request_seq": request["seq"],
            "command": request["command"], "success": true, "body": body,
        })
        .to_string();
        adapter
            .get_mut()
            .write_all(format!("Content-Length: {}\r\n\r\n{response}", response.len()).as_bytes())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn requests_and_reverse_replies_share_wire_sequence() {
        let (client, _incoming, adapter) = connection();
        let mut adapter = BufReader::new(adapter);
        let first = client.call::<requests::Threads>(None);
        let sender = client.request_sender();
        let abandoned = sender.call::<requests::Threads>(None);
        drop(abandoned);
        let second = tokio::spawn(sender.call::<requests::Threads>(None));
        let second_request = read_message(&mut adapter).await;
        assert_eq!(second_request["seq"], 1);

        client.reply(700, "runInTerminal", Ok(json!({}))).unwrap();
        client
            .reply(701, "startDebugging", Err(Error::Unhandled))
            .unwrap();
        for (sequence, request_sequence, success) in [(2, 700, true), (3, 701, false)] {
            let reply = read_message(&mut adapter).await;
            assert_eq!(reply["type"], "response");
            assert_eq!(reply["seq"], sequence);
            assert_eq!(reply["request_seq"], request_sequence);
            assert_eq!(reply["success"], success);
        }

        let first = tokio::spawn(first);
        let first_request = read_message(&mut adapter).await;
        assert_eq!(first_request["seq"], 4);
        // Reply out of order and without adapter sequence numbers, as some adapters do.
        reply_to(&mut adapter, &first_request, json!({"value": "first"})).await;
        reply_to(&mut adapter, &second_request, json!({"value": "second"})).await;
        assert_eq!(first.await.unwrap().unwrap()["value"], "first");
        assert_eq!(second.await.unwrap().unwrap()["value"], "second");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_senders_preserve_wire_order() {
        let (client, _incoming, adapter) = connection();
        let mut adapter = BufReader::new(adapter);
        let mut tasks = Vec::new();
        for _ in 0..32 {
            tasks.push(tokio::spawn(
                client.request_sender().call::<requests::Threads>(None),
            ));
        }
        for sequence in 1..=32 {
            let request = read_message(&mut adapter).await;
            assert_eq!(request["seq"], sequence);
            reply_to(&mut adapter, &request, json!({})).await;
        }
        for task in tasks {
            assert!(task.await.unwrap().is_ok());
        }
    }

    #[tokio::test]
    async fn failed_requests_preserve_adapter_details_and_display_readable_errors() {
        for (message, body, detail) in [
            (Some("notStopped"), None, "notStopped"),
            (
                None,
                Some(
                    json!({"error": {"id": 42, "format": "Cannot read {name}: {reason}", "variables": {"name": "{reason}", "reason": "unavailable"}}}),
                ),
                "Cannot read {reason}: unavailable",
            ),
            (
                Some("short error"),
                Some(json!({"error": {"format": "Unknown {missing}"}})),
                "Unknown {missing}",
            ),
            (
                Some("fallback"),
                Some(json!({"error": {"format": 42}})),
                "fallback",
            ),
            (
                Some("fallback"),
                Some(json!({"error": {"format": " "}})),
                "fallback",
            ),
            (
                None,
                Some(json!({"unexpected": true})),
                "adapter returned no error details",
            ),
            (None, None, "adapter returned no error details"),
        ] {
            let (client, _incoming, adapter) = connection();
            let mut adapter = BufReader::new(adapter);
            let request = tokio::spawn(client.threads());
            let envelope = read_message(&mut adapter).await;
            let response = json!({
                "type": "response", "request_seq": envelope["seq"],
                "command": "threads", "success": false, "message": message, "body": body,
            })
            .to_string();
            adapter
                .get_mut()
                .write_all(
                    format!("Content-Length: {}\r\n\r\n{response}", response.len()).as_bytes(),
                )
                .await
                .unwrap();
            let error = request.await.unwrap().unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("threads request failed: {detail}")
            );
            let Error::Request(error) = error else {
                panic!("expected a structured request failure")
            };
            assert_eq!(error.command, "threads");
            assert_eq!(error.request_seq, envelope["seq"].as_u64().unwrap());
            assert_eq!(error.message.as_deref(), message);
            assert_eq!(error.body, body);
            // Rejection does not close the session or prevent a later successful request.
            let retry = tokio::spawn(client.threads());
            let envelope = read_message(&mut adapter).await;
            reply_to(&mut adapter, &envelope, json!({"threads": []})).await;
            assert!(retry.await.unwrap().unwrap().threads.is_empty());
        }
    }

    #[tokio::test]
    async fn standard_acknowledgements_validate_successful_response_bodies() {
        for body in ["null", "{}", "42"] {
            let (client, _incoming, mut adapter) = connection();
            let request = tokio::spawn(client.next(ThreadId::default()));
            let mut bytes = [0; 4096];
            assert!(adapter.read(&mut bytes).await.unwrap() > 0);
            let response = format!(
                r#"{{"seq":1,"type":"response","request_seq":1,"success":true,"command":"next","body":{body}}}"#
            );
            adapter
                .write_all(
                    format!("Content-Length: {}\r\n\r\n{response}", response.len()).as_bytes(),
                )
                .await
                .unwrap();
            let result = time::timeout(time::Duration::from_secs(5), request)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.is_ok(), body != "42", "{body}: {result:?}");
        }
    }

    #[tokio::test]
    async fn eof_finishes_pending_requests_and_rejects_new_requests() {
        let (client, mut incoming, mut adapter) = connection();
        let sender = client.request_sender();
        let request =
            tokio::spawn(sender.call::<requests::Threads>(Some(requests::ThreadsArguments {})));
        let mut bytes = [0; 4096];
        assert!(adapter.read(&mut bytes).await.unwrap() > 0);
        drop(adapter);
        assert!(matches!(
            time::timeout(time::Duration::from_secs(5), incoming.recv())
                .await
                .unwrap(),
            Some((_, ClientEvent::Closed { error: None }))
        ));
        assert!(matches!(
            time::timeout(time::Duration::from_secs(5), request)
                .await
                .unwrap()
                .unwrap(),
            Err(Error::StreamClosed)
        ));
        assert!(matches!(
            sender.call::<requests::Threads>(None).await,
            Err(Error::StreamClosed)
        ));
        assert!(incoming.recv().await.is_none());
    }

    #[tokio::test]
    async fn dropping_the_owner_or_receiver_closes_io_despite_retained_request_senders() {
        for drop_owner in [true, false] {
            let (client, incoming, mut adapter) = connection();
            let sender = client.request_sender();
            let (mut client, mut incoming) = (Some(client), Some(incoming));
            if drop_owner {
                client.take();
            } else {
                incoming.take();
            }
            let mut byte = [0];
            assert_eq!(
                time::timeout(time::Duration::from_secs(5), adapter.read(&mut byte))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
            assert!(matches!(
                sender.call::<requests::Threads>(None).await,
                Err(Error::StreamClosed)
            ));
        }
    }

    #[tokio::test]
    async fn stderr_eof_does_not_close_the_protocol_connection() {
        let (client, mut adapter) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let (_client, mut incoming) = Client::streams(
            Box::new(BufReader::new(read)),
            Box::new(write),
            Some(Box::new(tokio::io::empty())),
            DebugAdapterId::default(),
            None,
        )
        .unwrap();
        let body = r#"{"type":"event","event":"initialized"}"#;
        adapter
            .write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
            .await
            .unwrap();
        assert!(matches!(
            incoming.recv().await,
            Some((_, ClientEvent::Message(Payload::Event(_))))
        ));
    }
}
