//! Startup uses controlled wire replies so cancellation and queued-result races are explicit.
use super::*;

struct Adapter {
    socket: std::net::SocketAddr,
    wire: mpsc::UnboundedReceiver<Value>,
    outgoing: mpsc::UnboundedSender<Value>,
}

impl Adapter {
    async fn new() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let socket = listener.local_addr()?;
        let (wire_tx, wire) = mpsc::unbounded_channel();
        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let (read, mut write) = stream.into_split();
            tokio::spawn(async move {
                let mut read = BufReader::new(read);
                loop {
                    let mut line = String::new();
                    if read.read_line(&mut line).await? == 0 {
                        return anyhow::Ok(());
                    }
                    let length: usize = line
                        .trim()
                        .strip_prefix("Content-Length: ")
                        .context("missing length")?
                        .parse()?;
                    line.clear();
                    read.read_line(&mut line).await?;
                    let mut bytes = vec![0; length];
                    read.read_exact(&mut bytes).await?;
                    if wire_tx.send(serde_json::from_slice(&bytes)?).is_err() {
                        return Ok(());
                    }
                }
            });
            while let Some(message) = outgoing_rx.recv().await {
                let bytes = serde_json::to_vec(&message)?;
                write
                    .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
                    .await?;
                write.write_all(&bytes).await?;
            }
            anyhow::Ok(())
        });
        Ok(Self {
            socket,
            wire,
            outgoing,
        })
    }

    fn reply(&self, request: &Value, success: bool, body: Value) -> anyhow::Result<()> {
        self.outgoing
            .send(json!({"type":"response", "request_seq":request["seq"],
            "command":request["command"], "success":success, "body":body,
            "message":(!success).then_some("fixture startup failure")}))?;
        Ok(())
    }
}

fn config() -> dap::DebugAdapterConfig {
    dap::DebugAdapterConfig {
        name: "fixture".into(),
        transport: "tcp".into(),
        command: String::new(),
        args: Vec::new(),
        port_arg: None,
        templates: Vec::new(),
        quirks: Default::default(),
    }
}

async fn incoming(f: &mut Fixture) -> anyhow::Result<DebugAdapterId> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let view::editor::EditorEvent::DebuggerEvent((id, payload)) =
                f.editor.wait_event().await
            {
                f.editor.handle_debugger_event(id, payload).await;
                return id;
            }
        }
    })
    .await
    .map_err(Into::into)
}

async fn wait_for_connection(f: &mut Fixture, id: DebugAdapterId) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.editor.debugger().get_client(id).is_none() {
            anyhow::ensure!(
                f.editor.debugger().is_starting(id),
                "startup failed before connecting"
            );
            debugger_callback(f).await?;
        }
        anyhow::Ok(())
    })
    .await?
}

async fn callback(f: &mut Fixture) -> anyhow::Result<view::callbacks::EditorCallback> {
    tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await
        .context("startup callback timed out")?
        .context("missing startup callback")
}

#[tokio::test(flavor = "multi_thread")]
async fn initialization_serves_reverse_requests_and_defers_configuration() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    f.editor.debugger_mut().set_terminal_launcher(Launcher {
        calls: Default::default(),
        fail: false,
        gate: None,
    })?;
    let mut adapter = Adapter::new().await?;
    let id = f.editor.connect_debugger(Some(adapter.socket), config());
    assert!(f.editor.debugger().is_starting(id));
    assert!(!f.editor.debugger().is_empty());
    wait_for_connection(&mut f, id).await?;
    let initialize = message(&mut adapter.wire).await?;
    assert_eq!(initialize["command"], "initialize");
    adapter
        .outgoing
        .send(json!({"type":"event","event":"initialized"}))?;
    assert_eq!(incoming(&mut f).await?, id);
    assert!(f.editor.debugger().get_active_client().is_none());
    adapter
        .outgoing
        .send(json!({"type":"request","seq":91,"command":"runInTerminal",
        "arguments":{"cwd":"/workspace","args":["program"]}}))?;
    assert_eq!(incoming(&mut f).await?, id);
    debugger_callback(&mut f).await?;
    let reply = message(&mut adapter.wire).await?;
    assert_eq!(reply["request_seq"], 91);
    assert_eq!(reply["success"], true);
    adapter.reply(
        &initialize,
        true,
        json!({"supportsConfigurationDoneRequest":true}),
    )?;
    wait_for_startup(&mut f, id).await?;
    assert_eq!(f.editor.debugger().get_active_client().unwrap().id(), id);
    let configured = message(&mut adapter.wire).await?;
    assert_eq!(configured["command"], "configurationDone");
    adapter.reply(&configured, true, json!({}))?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canceled_connections_cannot_install_queued_transports() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let mut old = Adapter::new().await?;
    let id = f.editor.connect_debugger(Some(old.socket), config());
    let connected = callback(&mut f).await?;
    assert!(f.editor.debugger_mut().cancel_startup(id));
    let replacement = Adapter::new().await?;
    let new_id = f
        .editor
        .connect_debugger(Some(replacement.socket), config());
    assert_ne!(id, new_id);
    connected(&mut f.editor);
    assert!(f.editor.debugger().get_client(id).is_none());
    assert!(f.editor.debugger().is_starting(new_id));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), old.wire.recv())
            .await?
            .is_none()
    );
    assert!(f.editor.debugger_mut().cancel_startup(new_id));
    assert!(f.editor.debugger().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_launch_document_rejects_queued_initialization() -> anyhow::Result<()> {
    let mut f = Fixture::with_languages("", LAUNCH_LANGUAGES)?;
    let document = view::current_ref!(f.editor).1.id();
    let mut adapter = Adapter::new().await?;
    let id = f
        .editor
        .start_debugger(document, Some("run"), Some(adapter.socket), None)?;
    wait_for_connection(&mut f, id).await?;
    let initialize = message(&mut adapter.wire)
        .await
        .context("initialize request missing")?;
    adapter.reply(&initialize, true, json!({}))?;
    let initialized = callback(&mut f).await?;
    assert!(f.editor.close_document(document, true).is_ok());
    f.editor.set_status("document closed");
    initialized(&mut f.editor);
    assert!(f.editor.debugger().get_client(id).is_none());
    assert!(!f.editor.debugger().is_starting(id));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), adapter.wire.recv())
            .await
            .context("closed adapter remained connected")?
            .is_none()
    );
    assert_eq!(f.editor.get_status().unwrap().0.as_ref(), "document closed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn termination_cancels_initialization_without_waiting_for_the_adapter() -> anyhow::Result<()>
{
    let mut f = Fixture::new("")?;
    let mut adapter = Adapter::new().await?;
    let id = f.editor.connect_debugger(Some(adapter.socket), config());
    wait_for_connection(&mut f, id).await?;
    assert_eq!(message(&mut adapter.wire).await?["command"], "initialize");
    f.editor
        .debugger_command(view::debugger::DebuggerCommand::Terminate)?;
    assert!(!f.editor.debugger().is_starting(id));
    assert!(f.editor.debugger().is_empty());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), adapter.wire.recv())
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn initialization_failures_release_the_adapter_and_report_the_error() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let mut adapter = Adapter::new().await?;
    let id = f.editor.connect_debugger(Some(adapter.socket), config());
    wait_for_connection(&mut f, id).await?;
    let initialize = message(&mut adapter.wire).await?;
    adapter.reply(&initialize, false, Value::Null)?;
    wait_for_startup(&mut f, id).await?;
    assert!(f.editor.debugger().is_empty());
    assert!(f.editor.is_err());
    assert!(f
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("Failed to start debugging"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn child_startup_replies_after_launch_and_cancels_with_its_parent() -> anyhow::Result<()> {
    for remove_parent in [false, true] {
        let mut f = Fixture::new("")?;
        let (parent, mut parent_wire) = adapter(&mut f).await?;
        message(&mut parent_wire).await?;
        let mut child = Adapter::new().await?;
        f.editor
            .debugger_mut()
            .get_client_mut(parent)
            .unwrap()
            .socket = Some(child.socket);
        tokio::time::timeout(
            Duration::from_secs(5),
            f.editor.handle_debugger_message(
                parent,
                request(
                    "startDebugging",
                    77,
                    json!({"request":"launch","configuration":{"program":"child"}}),
                ),
            ),
        )
        .await?;
        debugger_callback(&mut f).await?;
        let initialize = message(&mut child.wire).await?;
        assert_eq!(initialize["command"], "initialize");
        assert!(parent_wire.try_recv().is_err());
        if remove_parent {
            f.editor
                .handle_debugger_message(parent, debugger_event("terminated", Value::Null))
                .await;
            wait_for_adapter_removal(&mut f, parent).await?;
            assert!(f.editor.debugger().is_empty());
            assert!(
                tokio::time::timeout(Duration::from_secs(5), child.wire.recv())
                    .await?
                    .is_none()
            );
        } else {
            // The parent's transport remains usable while the child's initialization is pending.
            release_adapter_request(&f, parent).await?;
            assert_eq!(message(&mut parent_wire).await?["command"], "threads");
            child.reply(&initialize, true, json!({}))?;
            debugger_callback(&mut f).await?;
            let launch = message(&mut child.wire).await?;
            assert_eq!(launch["command"], "launch");
            assert_eq!(launch["arguments"]["program"], "child");
            assert!(parent_wire.try_recv().is_err());
            child.reply(&launch, true, json!({}))?;
            debugger_callback(&mut f).await?;
            let reply = message(&mut parent_wire).await?;
            assert_eq!(reply["request_seq"], 77);
            assert_eq!(reply["success"], true);
            assert!(parent_wire.try_recv().is_err());
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminated_adapters_do_not_finish_pending_initialization() -> anyhow::Result<()> {
    let mut f = Fixture::with_languages("", LAUNCH_LANGUAGES)?;
    let document = view::current_ref!(f.editor).1.id();
    let mut adapter = Adapter::new().await?;
    let id = f
        .editor
        .start_debugger(document, Some("run"), Some(adapter.socket), None)?;
    wait_for_connection(&mut f, id).await?;
    let initialize = message(&mut adapter.wire).await?;
    adapter.reply(&initialize, true, json!({}))?;
    let initialized = callback(&mut f).await?;
    f.editor
        .handle_debugger_message(id, debugger_event("terminated", Value::Null))
        .await;
    initialized(&mut f.editor);
    assert!(!f.editor.debugger().is_starting(id));
    assert!(f.editor.debugger().get_client(id).unwrap().caps.is_none());
    let disconnect = message(&mut adapter.wire).await?;
    assert_eq!(disconnect["command"], "disconnect");
    adapter.reply(&disconnect, true, json!({}))?;
    wait_for_adapter_removal(&mut f, id).await?;
    assert!(f.editor.debugger().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn initialization_order_preserves_selection_and_termination_targets_one_startup(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (running, _) = paused_adapter(&mut f).await?;
    let mut first = Adapter::new().await?;
    let first_id = f.editor.connect_debugger(Some(first.socket), config());
    wait_for_connection(&mut f, first_id).await?;
    let initialize = message(&mut first.wire).await?;
    let second = Adapter::new().await?;
    let second_id = f.editor.connect_debugger(Some(second.socket), config());
    f.editor.debugger_mut().select_session(running)?;
    first.reply(&initialize, true, json!({}))?;
    wait_for_startup(&mut f, first_id).await?;
    f.editor
        .handle_debugger_message(first_id, debugger_event("initialized", json!({})))
        .await;
    assert_eq!(
        f.editor.debugger().get_active_session().unwrap().id(),
        running
    );
    f.editor.debugger_mut().select_session(second_id)?;
    f.editor
        .debugger_command(view::debugger::DebuggerCommand::Terminate)?;
    assert!(f.editor.debugger().get_session(second_id).is_none());
    assert_eq!(
        f.editor.debugger().get_active_session().unwrap().id(),
        running
    );
    assert!(f.editor.debugger().get_session(first_id).is_some());
    assert!(f.editor.debugger().get_client(running).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn background_initialization_failures_preserve_the_selected_session() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (selected, _) = paused_adapter(&mut f).await?;
    let mut background = Adapter::new().await?;
    let id = f.editor.connect_debugger(Some(background.socket), config());
    wait_for_connection(&mut f, id).await?;
    let initialize = message(&mut background.wire).await?;
    f.editor.debugger_mut().select_session(selected)?;
    f.editor.set_status("selected session");
    background.reply(&initialize, false, Value::Null)?;
    wait_for_startup(&mut f, id).await?;
    assert!(f.editor.debugger().get_session(id).is_none());
    assert_eq!(
        f.editor.debugger().get_active_session().unwrap().id(),
        selected
    );
    assert_eq!(
        f.editor.get_status().unwrap().0.as_ref(),
        "selected session"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_eof_removes_its_session_and_restores_the_previous_selection() -> anyhow::Result<()>
{
    let mut f = Fixture::new("")?;
    let (previous, _) = paused_adapter(&mut f).await?;
    let mut adapter = Adapter::new().await?;
    let id = f.editor.connect_debugger(Some(adapter.socket), config());
    wait_for_connection(&mut f, id).await?;
    let initialize = message(&mut adapter.wire).await?;
    adapter.reply(&initialize, true, json!({}))?;
    wait_for_startup(&mut f, id).await?;
    drop(adapter);
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.editor.debugger().get_session(id).is_some() {
            incoming(&mut f).await?;
        }
        anyhow::Ok(())
    })
    .await??;
    assert_eq!(
        f.editor.debugger().get_active_session().unwrap().id(),
        previous
    );
    Ok(())
}
