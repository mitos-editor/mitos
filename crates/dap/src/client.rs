use crate::{
    requests::{DisconnectArguments, TerminateArguments},
    transport::{ClientEvent, Payload, Request, Response, Transport, TransportSender},
    DebugAdapterId, Error, Result,
};
use dap_types::*;

use serde_json::Value;

use anyhow::anyhow;
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncBufRead, AsyncWrite, BufReader, BufWriter},
    net::TcpStream,
    process::{Child, Command},
    sync::mpsc::{channel, UnboundedReceiver},
    time,
};

#[derive(Debug)]
/// One debug-adapter transport and its negotiated protocol configuration.
///
/// Construction returns a separate receiver for adapter events and reverse
/// requests. Callers must continue polling that receiver while awaiting typed
/// requests or an adapter can deadlock waiting for a reverse-request reply.
pub struct Client {
    id: DebugAdapterId,
    _process: Option<Child>,
    _transport: Transport,
    server_tx: TransportSender,
    request_counter: Arc<AtomicU64>,
    connection_type: Option<ConnectionType>,
    starting_request_args: Option<Value>,
    /// The socket address of the debugger, if using TCP transport.
    pub socket: Option<SocketAddr>,
    pub caps: Option<DebuggerCapabilities>,
    pub quirks: DebuggerQuirks,
    /// The config which was used to start this debugger.
    pub config: Option<DebugAdapterConfig>,
}

impl Client {
    /// Establish the configured transport, leaving initialization to the session owner.
    pub async fn connect(
        socket: Option<SocketAddr>,
        config: DebugAdapterConfig,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, ClientEvent)>)> {
        let (mut client, incoming) = match socket {
            Some(socket) => Self::tcp(socket, id).await?,
            None => {
                Self::process(
                    &config.transport,
                    &config.command,
                    config.args.iter().map(String::as_str).collect(),
                    config.port_arg.as_deref(),
                    id,
                )
                .await?
            }
        };
        client.quirks = config.quirks.clone();
        client.config = Some(config);
        Ok((client, incoming))
    }

    /// Spawns an adapter and communicates over its configured transport.
    ///
    /// The receiver tags every payload with `id`, allowing a registry to merge
    /// streams from multiple clients. TCP process transport requires
    /// `port_arg`; stdio ignores it.
    pub async fn process(
        transport: &str,
        command: &str,
        args: Vec<&str>,
        port_arg: Option<&str>,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, ClientEvent)>)> {
        if command.is_empty() {
            return Result::Err(Error::Other(anyhow!("Command not provided")));
        }
        match (transport, port_arg) {
            ("tcp", Some(port_arg)) => Self::tcp_process(command, args, port_arg, id).await,
            ("stdio", _) => Self::stdio(command, args, id),
            _ => Result::Err(Error::Other(anyhow!("Incorrect transport {}", transport))),
        }
    }

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
            request_counter: Arc::new(AtomicU64::new(0)),
            caps: None,
            connection_type: None,
            starting_request_args: None,
            socket: None,
            quirks: DebuggerQuirks::default(),
            config: None,
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

    async fn get_port() -> Option<u16> {
        Some(
            tokio::net::TcpListener::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                0,
            ))
            .await
            .ok()?
            .local_addr()
            .ok()?
            .port(),
        )
    }

    /// Returns the arguments used by the most recent launch or attach request.
    ///
    /// They are retained so adapters supporting restart can receive the same
    /// session configuration.
    pub fn starting_request_args(&self) -> Option<&Value> {
        self.starting_request_args.as_ref()
    }

    /// Spawns an adapter that listens on an ephemeral loopback TCP port.
    ///
    /// Every `{}` in `port_format` is replaced with the selected port. The
    /// resulting string is split on spaces and appended to `args`.
    pub async fn tcp_process(
        cmd: &str,
        args: Vec<&str>,
        port_format: &str,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, ClientEvent)>)> {
        let port = Self::get_port()
            .await
            .ok_or_else(|| anyhow!("Could not allocate a debug adapter port"))?;

        let process = Command::new(cmd)
            .args(args)
            .args(port_format.replace("{}", &port.to_string()).split(' '))
            // silence messages
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Startup cancellation and session removal own the adapter process too.
            .kill_on_drop(true)
            .spawn()?;

        // Wait for adapter to become ready for connection
        time::sleep(time::Duration::from_millis(500)).await;
        let socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
        let stream = TcpStream::connect(socket).await?;

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

    /// Returns whether this session was started by launch or attach.
    pub fn connection_type(&self) -> Option<ConnectionType> {
        self.connection_type
    }

    /// An owned transport handle for requests that must outlive a borrow of editor state.
    pub fn request_sender(&self) -> RequestSender {
        RequestSender {
            server_tx: self.server_tx.clone(),
            request_counter: self.request_counter.clone(),
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
                request_seq,
                command: command.to_owned(),
                success: true,
                message: None,
                body: Some(result),
            },
            Err(error) => Response {
                request_seq,
                command: command.to_owned(),
                success: false,
                message: Some(format!("{error:#}")),
                body: None,
            },
        };
        self.server_tx.send(Payload::Response(response))
    }

    /// Returns the negotiated capabilities.
    ///
    /// # Panics
    ///
    /// Panics until the owner installs the capabilities returned by [`Self::initialize`].
    pub fn capabilities(&self) -> &DebuggerCapabilities {
        self.caps.as_ref().expect("debugger not yet initialized!")
    }

    /// Return an owned initialization request. The owner installs its capabilities
    /// while continuing to serve incoming events and reverse requests.
    pub fn initialize(
        &self,
        adapter_id: String,
        supports_run_in_terminal: bool,
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
            supports_variable_paging: Some(false),
            supports_run_in_terminal_request: Some(supports_run_in_terminal),
            supports_memory_references: Some(false),
            supports_progress_reporting: Some(true),
            supports_invalidated_event: Some(false),
        };

        self.request_sender().request::<requests::Initialize>(args)
    }

    pub fn disconnect(
        &mut self,
        args: Option<DisconnectArguments>,
    ) -> impl Future<Output = Result<Value>> + use<> {
        self.connection_type = None;
        self.call::<requests::Disconnect>(args)
    }

    pub fn terminate(
        &mut self,
        args: Option<TerminateArguments>,
    ) -> impl Future<Output = Result<Value>> + use<> {
        self.connection_type = None;
        self.call::<requests::Terminate>(args)
    }

    pub fn launch(
        &mut self,
        args: serde_json::Value,
    ) -> impl Future<Output = Result<Value>> + use<> {
        self.connection_type = Some(ConnectionType::Launch);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Launch>(args)
    }

    pub fn attach(
        &mut self,
        args: serde_json::Value,
    ) -> impl Future<Output = Result<Value>> + use<> {
        self.connection_type = Some(ConnectionType::Attach);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Attach>(args)
    }

    pub fn restart(&self) -> impl Future<Output = Result<Value>> + use<> {
        let args = if let Some(args) = &self.starting_request_args {
            args.clone()
        } else {
            Value::Null
        };
        self.call::<requests::Restart>(args)
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

    pub async fn configuration_done(&self) -> Result<()> {
        if !self
            .caps
            .as_ref()
            .and_then(|caps| caps.supports_configuration_done_request)
            .unwrap_or(false)
        {
            return Ok(());
        }

        self.request::<requests::ConfigurationDone>(Some(requests::ConfigurationDoneArguments {}))
            .await
            .map(|_| ())
    }

    pub fn continue_thread(
        &self,
        thread_id: ThreadId,
    ) -> impl Future<Output = Result<Value>> + use<> {
        let args = requests::ContinueArguments { thread_id };

        self.call::<requests::Continue>(args)
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

    pub fn threads(&self) -> impl Future<Output = Result<Value>> + use<> {
        self.call::<requests::Threads>(Some(requests::ThreadsArguments {}))
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

    pub fn step_in(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> + use<> {
        let args = requests::StepInArguments {
            thread_id,
            target_id: None,
            granularity: None,
        };

        self.call::<requests::StepIn>(args)
    }

    pub fn step_out(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> + use<> {
        let args = requests::StepOutArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::StepOut>(args)
    }

    pub fn next(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> + use<> {
        let args = requests::NextArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::Next>(args)
    }

    pub fn pause(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> + use<> {
        let args = requests::PauseArguments { thread_id };

        self.call::<requests::Pause>(args)
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
    ) -> impl Future<Output = Result<Value>> + use<> {
        let args = requests::SetExceptionBreakpointsArguments { filters };

        self.call::<requests::SetExceptionBreakpoints>(args)
    }
}

/// Cloneable request transport. It carries no selected thread, frame, or editor state.
#[derive(Clone, Debug)]
pub struct RequestSender {
    server_tx: TransportSender,
    request_counter: Arc<AtomicU64>,
}

impl RequestSender {
    fn next_request_id(&self) -> u64 {
        // > The `seq` for the first message sent by a client or debug adapter
        // > is 1, and for each subsequent message is 1 greater than the
        // > previous message sent by that actor
        // <https://microsoft.github.io/debug-adapter-protocol/specification#Base_Protocol_ProtocolMessage>
        self.request_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

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
        let id = self.next_request_id();

        async move {
            use std::time::Duration;
            use tokio::time::timeout;

            let arguments = Some(serde_json::to_value(arguments)?);

            let (callback_tx, mut callback_rx) = channel(1);

            let req = Request {
                back_ch: Some(callback_tx),
                seq: id,
                command: R::COMMAND.to_string(),
                arguments,
            };

            server_tx.send(Payload::Request(req))?;

            // Dropping the wait releases transport bookkeeping, including on cancellation.
            let _pending = PendingRequest {
                sender: server_tx,
                sequence: id,
            };
            let response = timeout(Duration::from_secs(20), callback_rx.recv())
                .await
                .map_err(|_| Error::Timeout(id))? // return Timeout
                .ok_or(Error::StreamClosed)??;

            if !response.success {
                let message = response
                    .message
                    .clone()
                    .unwrap_or_else(|| "DAP request failed".to_string());
                return Err(Error::Other(anyhow!(message)));
            }

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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
