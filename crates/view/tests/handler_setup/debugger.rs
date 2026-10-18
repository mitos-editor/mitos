use super::Fixture;
use anyhow::Context as _;
use dap::{
    registry::DebugAdapterId,
    requests::{RunInTerminalArguments, RunInTerminalResponse},
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::mpsc,
};
use view::debugger::TerminalLauncher;

#[derive(Clone)]
struct Launcher {
    calls: Arc<Mutex<Vec<RunInTerminalArguments>>>,
    fail: bool,
    gate: Option<Arc<tokio::sync::Notify>>,
}
impl TerminalLauncher for Launcher {
    fn launch(
        &self,
        args: RunInTerminalArguments,
    ) -> BoxFuture<'static, dap::Result<RunInTerminalResponse>> {
        self.calls.lock().unwrap().push(args);
        let fail = self.fail;
        let gate = self.gate.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            if fail {
                return Err(anyhow::anyhow!("fixture launch failure").into());
            }
            Ok(RunInTerminalResponse {
                process_id: Some(42),
                shell_process_id: None,
            })
        })
    }
}

async fn adapter(
    f: &mut Fixture,
) -> anyhow::Result<(DebugAdapterId, mpsc::UnboundedReceiver<Value>)> {
    let (socket, receiver) = adapter_server(false).await?;
    let config = dap::DebugAdapterConfig {
        name: "fixture".into(),
        transport: "tcp".into(),
        command: String::new(),
        args: vec![],
        port_arg: None,
        templates: vec![],
        quirks: Default::default(),
    };
    let id = f
        .editor
        .debugger_mut()
        .start_client(Some(socket), &config)?;
    Ok((id, receiver))
}

async fn adapter_server(
    fail_launch: bool,
) -> anyhow::Result<(std::net::SocketAddr, mpsc::UnboundedReceiver<Value>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let socket = listener.local_addr()?;
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await?;
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let mut delayed = None;
        let mut held_breakpoint = None;
        let mut hold_breakpoints = true;
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
            let message: Value = serde_json::from_slice(&bytes)?;
            sender.send(message.clone()).ok();
            if message["type"] == "request" {
                if message["command"] == "evaluate" && message["arguments"]["expression"] == "wait"
                {
                    delayed = Some(message);
                    continue;
                }
                if message["command"] == "setBreakpoints"
                    && hold_breakpoints
                    && message["arguments"]["source"]["path"]
                        .as_str()
                        .is_some_and(|path| path.ends_with("held.rs"))
                {
                    hold_breakpoints = false;
                    held_breakpoint = Some(message);
                    continue;
                }
                let body = match message["command"].as_str().unwrap() {
                    "initialize" => {
                        json!({"supportsRestartRequest":true,"supportsTerminateRequest":true,"supportsConfigurationDoneRequest":true})
                    }
                    "setBreakpoints" => breakpoint_response(&message),
                    "threads" => {
                        json!({"threads":[{"id":1,"name":"main"},{"id":2,"name":"worker"}]})
                    }
                    "stackTrace" => {
                        let thread = message["arguments"]["threadId"].as_u64().unwrap();
                        json!({"stackFrames":[{"id":thread*100+1,"name":"first","line":1,"column":1},{"id":thread*100+2,"name":"second","line":2,"column":1}]})
                    }
                    "scopes" => {
                        json!({"scopes":[{"name":"locals","variablesReference":7,"expensive":false}]})
                    }
                    "variables" => {
                        json!({"variables":[{"name":"answer","value":"42","type":"int","variablesReference":0}]})
                    }
                    "evaluate" => {
                        json!({"result":message["arguments"]["expression"],"variablesReference":0})
                    }
                    "continue" => json!({"allThreadsContinued":true}),
                    _ => json!({}),
                };
                let response = serde_json::to_vec(&json!({"type":"response", "seq":1,
                    "request_seq":message["seq"], "command":message["command"], "success":!(fail_launch && message["command"] == "launch"), "body":body}))?;
                write
                    .write_all(format!("Content-Length: {}\r\n\r\n", response.len()).as_bytes())
                    .await?;
                write.write_all(&response).await?;
                if let Some(held) = held_breakpoint.take() {
                    let response = serde_json::to_vec(
                        &json!({"type":"response", "seq":2, "request_seq":held["seq"], "command":"setBreakpoints", "success":true, "body":breakpoint_response(&held)}),
                    )?;
                    write
                        .write_all(format!("Content-Length: {}\r\n\r\n", response.len()).as_bytes())
                        .await?;
                    write.write_all(&response).await?;
                }
                if let Some(delayed) = delayed.take() {
                    let response = serde_json::to_vec(&json!({"type":"response", "seq":2,
                        "request_seq":delayed["seq"], "command":"evaluate", "success":true,
                        "body":{"result":"late", "variablesReference":0}}))?;
                    write
                        .write_all(format!("Content-Length: {}\r\n\r\n", response.len()).as_bytes())
                        .await?;
                    write.write_all(&response).await?;
                }
            }
        }
    });
    Ok((socket, receiver))
}

fn breakpoint_response(message: &Value) -> Value {
    let breakpoints: Vec<_> = message["arguments"]["breakpoints"].as_array().unwrap().iter().map(|breakpoint| json!({"id":1000 + breakpoint["line"].as_u64().unwrap(), "verified":true, "line":breakpoint["line"]})).collect();
    json!({"breakpoints":breakpoints})
}

async fn message(receiver: &mut mpsc::UnboundedReceiver<Value>) -> anyhow::Result<Value> {
    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await?
        .context("adapter stream closed")
}

fn request(command: &str, seq: u64, arguments: Value) -> dap::Payload {
    serde_json::from_value(
        json!({"type":"request","seq":seq,"command":command,"arguments":arguments}),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_terminal_requests_use_the_frontend_and_complete_once() -> anyhow::Result<()> {
    for fail in [false, true] {
        let mut f = Fixture::new("text\n")?;
        let launcher = Launcher {
            calls: Default::default(),
            fail,
            gate: None,
        };
        let calls = launcher.calls.clone();
        f.editor.debugger_mut().set_terminal_launcher(launcher)?;
        let (id, mut wire) = adapter(&mut f).await?;
        assert_eq!(
            message(&mut wire).await?["arguments"]["supportsRunInTerminalRequest"],
            true
        );
        let arguments = json!({"kind":"external","cwd":"/workspace","args":["program","literal argument"],
            "env":{"MITOS_TEST_VALUE":"value","MITOS_TEST_REMOVE":null}});
        f.editor
            .handle_debugger_message(id, request("runInTerminal", 91, arguments.clone()))
            .await;
        tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
            .await?
            .context("terminal result callback missing")?(&mut f.editor);
        let reply = message(&mut wire).await?;
        assert_eq!(reply["request_seq"], 91);
        assert_eq!(reply["success"], !fail);
        if fail {
            assert!(reply["message"]
                .as_str()
                .unwrap()
                .contains("fixture launch failure"));
        } else {
            assert_eq!(reply["body"]["processId"], 42);
        }
        assert_eq!(serde_json::to_value(&calls.lock().unwrap()[0])?, arguments);
        f.editor
            .debugger()
            .get_client(id)
            .unwrap()
            .request::<dap::requests::Threads>(Some(dap::requests::ThreadsArguments {}))
            .await?;
        assert_eq!(message(&mut wire).await?["command"], "threads");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_and_invalid_debugger_requests_receive_error_replies() -> anyhow::Result<()> {
    let mut f = Fixture::new("text\n")?;
    let (id, mut wire) = adapter(&mut f).await?;
    assert_eq!(
        message(&mut wire).await?["arguments"]["supportsRunInTerminalRequest"],
        false
    );
    f.editor.debugger_mut().get_client_mut(id).unwrap().socket = None;
    for (index, (command, arguments)) in [
        ("runInTerminal", json!({"cwd":"","args":["program"]})),
        ("runInTerminal", json!({"bad":"arguments"})),
        (
            "startDebugging",
            json!({"request":"launch","configuration":{}}),
        ),
        ("unknownRequest", json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let seq = index as u64 + 100;
        f.editor
            .handle_debugger_message(id, request(command, seq, arguments.clone()))
            .await;
        if command == "runInTerminal" && arguments.get("args").is_some() {
            tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
                .await?
                .context("terminal result callback missing")?(&mut f.editor);
        }
        let reply = message(&mut wire).await?;
        assert_eq!(reply["request_seq"], seq);
        assert_eq!(reply["command"], command);
        assert_eq!(reply["success"], false);
        assert!(reply["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()));
    }
    let backend = Launcher {
        calls: Default::default(),
        fail: false,
        gate: None,
    };
    assert!(f
        .editor
        .debugger_mut()
        .set_terminal_launcher(backend)
        .is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_from_unknown_adapters_do_not_launch_terminals() -> anyhow::Result<()> {
    let mut f = Fixture::new("text\n")?;
    let launcher = Launcher {
        calls: Default::default(),
        fail: false,
        gate: None,
    };
    let calls = launcher.calls.clone();
    f.editor.debugger_mut().set_terminal_launcher(launcher)?;
    assert!(
        !f.editor
            .handle_debugger_message(
                DebugAdapterId::default(),
                request("runInTerminal", 1, json!({"cwd":"","args":["program"]}))
            )
            .await
    );
    assert!(calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_startup_keeps_the_editor_available() -> anyhow::Result<()> {
    let mut f = Fixture::new("text\n")?;
    let gate = Arc::new(tokio::sync::Notify::new());
    f.editor.debugger_mut().set_terminal_launcher(Launcher {
        calls: Default::default(),
        fail: false,
        gate: Some(gate.clone()),
    })?;
    let (id, mut wire) = adapter(&mut f).await?;
    message(&mut wire).await?;
    tokio::time::timeout(
        Duration::from_secs(5),
        f.editor.handle_debugger_message(
            id,
            request("runInTerminal", 92, json!({"cwd":"","args":["program"]})),
        ),
    )
    .await?;
    f.editor.set_status("editor is available");
    assert!(f.callbacks.try_recv().is_err());
    gate.notify_one();
    tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .context("terminal result callback missing")?(&mut f.editor);
    let reply = message(&mut wire).await?;
    assert_eq!(reply["request_seq"], 92);
    assert_eq!(reply["success"], true);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_results_from_removed_adapters_do_not_change_editor_state() -> anyhow::Result<()> {
    let mut f = Fixture::new("text\n")?;
    let gate = Arc::new(tokio::sync::Notify::new());
    f.editor.debugger_mut().set_terminal_launcher(Launcher {
        calls: Default::default(),
        fail: true,
        gate: Some(gate.clone()),
    })?;
    let (id, mut wire) = adapter(&mut f).await?;
    message(&mut wire).await?;
    f.editor
        .handle_debugger_message(
            id,
            request("runInTerminal", 93, json!({"cwd":"","args":["program"]})),
        )
        .await;
    let terminated = serde_json::from_value(json!({"type":"event","seq":94,"event":"terminated"}))?;
    f.editor.handle_debugger_message(id, terminated).await;
    assert_eq!(message(&mut wire).await?["command"], "disconnect");
    assert!(f.editor.debugger().get_client(id).is_none());
    f.editor.set_status("session is closed");
    gate.notify_one();
    tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .context("terminal result callback missing")?(&mut f.editor);
    assert_eq!(
        f.editor.get_status().unwrap().0.as_ref(),
        "session is closed"
    );
    Ok(())
}

const LAUNCH_LANGUAGES: &str = r#"
[[language]]
name = "debug-test"
scope = "source.debug-test"
file-types = ["words"]
roots = []
[language.debugger]
name = "fixture"
transport = "tcp"
[[language.debugger.templates]]
name = "run"
request = "launch"
args = { program = "{0}", port = "{1}", nested = ["{0}", true] }
[[language.debugger.templates]]
name = "unsupported"
request = "unknown"
args = {}
"#;

#[tokio::test(flavor = "multi_thread")]
async fn debugger_launch_validates_configuration_before_connecting_and_preserves_origin(
) -> anyhow::Result<()> {
    let mut f = Fixture::with_languages("", LAUNCH_LANGUAGES)?;
    let document = view::current_ref!(f.editor).1.id();
    let (socket, mut wire) = adapter_server(false).await?;
    for name in ["missing", "unsupported"] {
        let error = f
            .editor
            .start_debugger(document, Some(name), Some(socket), None)
            .unwrap_err();
        assert!(!error.to_string().contains("Failed to start debug client"));
        assert!(wire.try_recv().is_err());
    }
    f.editor.new_file(view::editor::Action::Replace);
    let id = f.editor.start_debugger(
        document,
        Some("run"),
        Some(socket),
        Some(&["program name".into(), "123".into()]),
    )?;
    assert_eq!(message(&mut wire).await?["command"], "initialize");
    let launch = message(&mut wire).await?;
    assert_eq!(launch["command"], "launch");
    assert_eq!(launch["arguments"]["program"], "program name");
    assert_eq!(launch["arguments"]["port"], 123);
    assert_eq!(launch["arguments"]["nested"], json!(["program name", true]));
    assert!(f.editor.debugger().get_client(id).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_debugger_launch_removes_its_adapter_and_reports_the_error() -> anyhow::Result<()> {
    let mut f = Fixture::with_languages("", LAUNCH_LANGUAGES)?;
    let document = view::current_ref!(f.editor).1.id();
    let (socket, mut wire) = adapter_server(true).await?;
    let id = f
        .editor
        .start_debugger(document, Some("run"), Some(socket), None)?;
    assert_eq!(message(&mut wire).await?["command"], "initialize");
    assert_eq!(message(&mut wire).await?["command"], "launch");
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.editor.debugger().get_client(id).is_some() {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    assert!(f.editor.is_err());
    Ok(())
}

fn debugger_event(name: &str, body: Value) -> dap::Payload {
    serde_json::from_value(json!({"type":"event", "seq":1, "event":name, "body":body})).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn breakpoint_configuration_coalesces_changes_before_finishing_startup() -> anyhow::Result<()>
{
    let mut f = Fixture::new("")?;
    let path = f.dir.path().join("held.rs");
    f.editor.toggle_breakpoints(path.clone(), &[2])?;
    let (id, mut wire) = adapter(&mut f).await?;
    command(&mut wire, "initialize").await?;
    tokio::time::timeout(
        Duration::from_secs(5),
        f.editor
            .handle_debugger_message(id, debugger_event("initialized", json!({}))),
    )
    .await?;
    let first = command(&mut wire, "setBreakpoints").await?;
    assert_eq!(first["arguments"]["breakpoints"][0]["line"], 3);
    f.editor.toggle_breakpoints(path.clone(), &[2, 4])?;
    assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 4);
    f.editor
        .handle_debugger_message(
            id,
            debugger_event(
                "continued",
                json!({"threadId":1,"allThreadsContinued":true}),
            ),
        )
        .await;
    // A thread request releases the adapter's deliberately held response. The
    // editor remains free to process requests while configuration is pending.
    let _ = f
        .editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f
            .editor
            .debugger()
            .breakpoint_locations(&path)
            .next()
            .unwrap()
            .verified
        {
            f.callbacks
                .recv()
                .await
                .context("missing breakpoint callback")?(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await??;
    let latest = command(&mut wire, "setBreakpoints").await?;
    assert_eq!(latest["arguments"]["breakpoints"], json!([{"line":5}]));
    assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 4);
    assert_eq!(
        f.editor
            .debugger()
            .breakpoint_binding(f.editor.debugger().breakpoints(&path)[0].identity())
            .and_then(|binding| binding.id),
        Some(1005)
    );
    command(&mut wire, "configurationDone").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn breakpoint_replies_from_removed_sessions_are_discarded() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let path = f.dir.path().join("held.rs");
    f.editor.toggle_breakpoints(path.clone(), &[2])?;
    let (id, mut wire) = adapter(&mut f).await?;
    command(&mut wire, "initialize").await?;
    f.editor
        .handle_debugger_message(id, debugger_event("initialized", json!({})))
        .await;
    command(&mut wire, "setBreakpoints").await?;
    let _ = f.editor.debugger_threads()?.run().await;
    let callback = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .context("missing breakpoint callback")?;
    f.editor
        .handle_debugger_message(id, debugger_event("terminated", json!({})))
        .await;
    callback(&mut f.editor);
    assert!(
        !f.editor
            .debugger()
            .breakpoint_locations(&path)
            .next()
            .unwrap()
            .verified
    );
    assert!(f.editor.debugger().get_client(id).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inactive_adapter_breakpoint_replies_preserve_the_active_session() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let path = f.dir.path().join("held.rs");
    f.editor.toggle_breakpoints(path.clone(), &[2])?;
    let (old, mut wire) = adapter(&mut f).await?;
    command(&mut wire, "initialize").await?;
    f.editor
        .handle_debugger_message(old, debugger_event("initialized", json!({})))
        .await;
    command(&mut wire, "setBreakpoints").await?;
    let _ = f.editor.debugger_threads()?.run().await;
    let callback = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .context("missing breakpoint callback")?;
    let (active, mut active_wire) = adapter(&mut f).await?;
    command(&mut active_wire, "initialize").await?;
    f.editor
        .handle_debugger_message(active, debugger_event("initialized", json!({})))
        .await;
    command(&mut active_wire, "setBreakpoints").await?;
    callback(&mut f.editor);
    assert!(f.editor.debugger().get_client(old).is_some());
    assert_eq!(
        f.editor.debugger().get_active_client().unwrap().id(),
        active
    );
    assert!(
        !f.editor
            .debugger()
            .breakpoint_locations(&path)
            .next()
            .unwrap()
            .verified
    );
    assert_eq!(
        f.editor
            .debugger()
            .breakpoint_binding(f.editor.debugger().breakpoints(&path)[0].identity())
            .and_then(|binding| binding.id),
        None
    );
    Ok(())
}

async fn paused_adapter(
    f: &mut Fixture,
) -> anyhow::Result<(DebugAdapterId, mpsc::UnboundedReceiver<Value>)> {
    let (id, wire) = adapter(f).await?;
    f.editor
        .handle_debugger_message(id, debugger_event("initialized", json!({})))
        .await;
    stop_adapter(f, id).await;
    while let Ok(callback) = f.callbacks.try_recv() {
        callback(&mut f.editor);
    }
    Ok((id, wire))
}

async fn stop_adapter(f: &mut Fixture, id: DebugAdapterId) {
    f.editor
        .handle_debugger_message(
            id,
            debugger_event("stopped", json!({"reason":"breakpoint", "threadId":1})),
        )
        .await;
}

async fn command(
    wire: &mut mpsc::UnboundedReceiver<Value>,
    expected: &str,
) -> anyhow::Result<Value> {
    loop {
        let message = message(wire).await?;
        if message["command"] == expected {
            return Ok(message);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn debugger_inspection_returns_typed_results_and_rejects_other_editors() -> anyhow::Result<()>
{
    let mut first = Fixture::new("")?;
    let mut second = Fixture::new("")?;
    let (id, _) = paused_adapter(&mut first).await?;
    let (other, _) = paused_adapter(&mut second).await?;
    assert_eq!(id, other);
    let variables = first.editor.debugger_variables()?;
    let evaluation = first.editor.debugger_evaluate("expression".into())?;
    first.editor.set_status("editor is available");
    let (variables, evaluation) = tokio::join!(variables.run(), evaluation.run());
    let variables = variables.unwrap();
    assert!(!variables.context().is_current(&second.editor));
    let scopes = variables.into_result(&first.editor).unwrap()?;
    assert_eq!(scopes[0].name, "locals");
    assert_eq!(scopes[0].variables[0].value, "42");
    assert!(evaluation.unwrap().into_result(&second.editor).is_none());
    let old = first.editor.debugger_variables()?.run().await.unwrap();
    first
        .editor
        .handle_debugger_message(id, debugger_event("continued", json!({"threadId":1})))
        .await;
    stop_adapter(&mut first, id).await;
    assert!(old.into_result(&first.editor).is_none());
    assert_eq!(
        first
            .editor
            .debugger()
            .get_active_client()
            .unwrap()
            .current_stack_frame()
            .unwrap()
            .id,
        101
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn superseded_debugger_requests_cancel_waits_and_discard_late_replies() -> anyhow::Result<()>
{
    use futures_util::FutureExt as _;
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter(&mut f).await?;
    let old = f.editor.debugger_evaluate("wait".into())?;
    let waiting = tokio::spawn(old.run());
    command(&mut wire, "evaluate").await?;
    let next = f.editor.debugger_evaluate("fresh".into())?;
    assert!(tokio::time::timeout(Duration::from_secs(5), waiting)
        .await??
        .is_none());
    let update = next.run().await.unwrap();
    assert_eq!(update.into_result(&f.editor).unwrap()?.result, "fresh");
    // A subsequent response flushes the server's preceding late reply through the transport.
    let sender = f.editor.debugger().get_client(id).unwrap().request_sender();
    sender
        .request::<dap::requests::Threads>(Some(dap::requests::ThreadsArguments {}))
        .await?;
    if let Some(event) = f.editor.wait_event().now_or_never() {
        assert!(!matches!(
            event,
            view::editor::EditorEvent::DebuggerEvent(_)
        ));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_debugger_controls_preserve_new_stops_and_replacement_sessions() -> anyhow::Result<()>
{
    use view::debugger::DebuggerCommand;
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter(&mut f).await?;
    let stopped_thread = f.editor.debugger().get_client(id).unwrap().thread_id;
    f.editor.debugger_command(DebuggerCommand::Continue)?;
    command(&mut wire, "continue").await?;
    let old = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .unwrap();
    stop_adapter(&mut f, id).await;
    old(&mut f.editor);
    assert_eq!(
        f.editor.debugger().get_client(id).unwrap().thread_id,
        stopped_thread
    );
    f.editor.debugger_command(DebuggerCommand::Continue)?;
    command(&mut wire, "continue").await?;
    let old = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .unwrap();
    f.editor
        .handle_debugger_message(id, debugger_event("terminated", Value::Null))
        .await;
    let (replacement, mut wire) = paused_adapter(&mut f).await?;
    assert_ne!(id, replacement);
    old(&mut f.editor);
    assert_eq!(
        f.editor
            .debugger()
            .get_client(replacement)
            .unwrap()
            .thread_id,
        stopped_thread
    );
    f.editor.debugger_command(DebuggerCommand::Continue)?;
    command(&mut wire, "continue").await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while f
            .editor
            .debugger()
            .get_client(replacement)
            .unwrap()
            .thread_id
            .is_some()
        {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn debugger_picker_contexts_preserve_new_thread_and_frame_selections() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter(&mut f).await?;
    let threads = f.editor.debugger_threads()?.run().await.unwrap();
    let context = threads.context();
    let listed = threads.into_result(&f.editor).unwrap()?;
    assert_eq!(listed.len(), 2);
    let (main, worker) = (listed[0].id, listed[1].id);
    f.editor.select_debugger_thread(&context, worker)?;
    command(&mut wire, "stackTrace").await?;
    let old = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .unwrap();
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 1)?;
    old(&mut f.editor);
    f.editor.select_debugger_frame(&context, 0)?;
    let client = f.editor.debugger().get_client(id).unwrap();
    assert_eq!(client.thread_id, Some(main));
    assert_eq!(client.current_stack_frame().unwrap().id, 102);
    let threads = f.editor.debugger_threads()?.run().await.unwrap();
    f.editor
        .select_debugger_thread(&threads.context(), worker)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.editor.debugger().get_client(id).unwrap().thread_id != Some(worker) {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    assert_eq!(
        f.editor
            .debugger()
            .get_client(id)
            .unwrap()
            .current_stack_frame()
            .unwrap()
            .id,
        201
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inactive_adapter_events_preserve_the_active_session() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (inactive, _) = paused_adapter(&mut f).await?;
    let (active, _) = paused_adapter(&mut f).await?;
    f.editor
        .debugger_command(view::debugger::DebuggerCommand::Continue)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while f
            .editor
            .debugger()
            .get_client(active)
            .unwrap()
            .thread_id
            .is_some()
        {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    stop_adapter(&mut f, inactive).await;
    assert_eq!(
        f.editor.debugger().get_active_client().unwrap().id(),
        active
    );
    assert!(f
        .editor
        .debugger()
        .get_client(active)
        .unwrap()
        .thread_id
        .is_none());
    f.editor
        .handle_debugger_message(inactive, debugger_event("terminated", Value::Null))
        .await;
    assert!(f.editor.debugger().get_client(inactive).is_none());
    assert_eq!(
        f.editor.debugger().get_active_client().unwrap().id(),
        active
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn breakpoint_events_are_scoped_to_their_adapter() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let path = f.dir.path().join("main.rs");
    f.editor.toggle_breakpoints(path.clone(), &[2])?;
    let identity = f.editor.debugger().breakpoints(&path)[0].identity();
    let (first, mut first_wire) = adapter(&mut f).await?;
    f.editor
        .handle_debugger_message(first, debugger_event("initialized", json!({})))
        .await;
    wait_for_breakpoint(&mut f, identity).await?;
    command(&mut first_wire, "configurationDone").await?;
    let (second, mut second_wire) = adapter(&mut f).await?;
    f.editor
        .handle_debugger_message(second, debugger_event("initialized", json!({})))
        .await;
    wait_for_breakpoint(&mut f, identity).await?;
    command(&mut second_wire, "configurationDone").await?;
    // Both adapters assigned 1003. Neither an inactive event nor termination
    // can change the second adapter's binding or the requested source line.
    f.editor
        .handle_debugger_message(
            first,
            debugger_event(
                "breakpoint",
                json!({"reason":"changed", "breakpoint":{"id":1003,"verified":false,"line":42}}),
            ),
        )
        .await;
    assert_eq!(
        f.editor
            .debugger()
            .breakpoint_binding(identity)
            .unwrap()
            .line,
        2
    );
    assert!(
        f.editor
            .debugger()
            .breakpoint_binding(identity)
            .unwrap()
            .verified
    );
    f.editor
        .handle_debugger_message(
            first,
            debugger_event(
                "breakpoint",
                json!({"reason":"removed", "breakpoint":{"id":1003,"verified":false}}),
            ),
        )
        .await;
    f.editor
        .handle_debugger_message(first, debugger_event("terminated", Value::Null))
        .await;
    assert!(
        f.editor
            .debugger()
            .breakpoint_binding(identity)
            .unwrap()
            .verified
    );
    f.editor
        .handle_debugger_message(
            second,
            debugger_event(
                "breakpoint",
                json!({"reason":"changed", "breakpoint":{"id":1003,"verified":true,"line":10}}),
            ),
        )
        .await;
    assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 2);
    assert_eq!(
        f.editor
            .debugger()
            .breakpoint_at_line(&path, 9)
            .unwrap()
            .identity(),
        identity
    );
    f.editor.toggle_breakpoints(path.clone(), &[9])?;
    assert!(f.editor.debugger().breakpoints(&path).is_empty());
    let sent = command(&mut second_wire, "setBreakpoints").await?;
    assert_eq!(sent["arguments"]["breakpoints"], json!([]));
    Ok(())
}

async fn wait_for_breakpoint(
    f: &mut Fixture,
    identity: view::debugger::BreakpointId,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f
            .editor
            .debugger()
            .breakpoint_binding(identity)
            .is_some_and(|binding| binding.verified)
        {
            f.callbacks
                .recv()
                .await
                .context("missing breakpoint callback")?(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_created_breakpoints_end_with_the_session() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let path = f.dir.path().join("main.rs");
    let (id, _) = paused_adapter(&mut f).await?;
    f.editor.handle_debugger_message(id, debugger_event("breakpoint", json!({"reason":"new", "breakpoint":{"id":7,"verified":true,"line":8,"source":{"path":path}}}))).await;
    assert!(f.editor.debugger().breakpoints(&path).is_empty());
    assert_eq!(
        f.editor
            .debugger()
            .breakpoint_locations(&path)
            .next()
            .unwrap()
            .line,
        7
    );
    f.editor
        .handle_debugger_message(id, debugger_event("terminated", Value::Null))
        .await;
    assert!(f
        .editor
        .debugger()
        .breakpoint_locations(&path)
        .next()
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn source_breakpoint_edits_reach_every_configured_adapter() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let path = f.dir.path().join("main.rs");
    let (first, mut first_wire) = paused_adapter(&mut f).await?;
    let (second, mut second_wire) = paused_adapter(&mut f).await?;
    f.editor.toggle_breakpoints(path.clone(), &[2])?;
    for wire in [&mut first_wire, &mut second_wire] {
        let sent = command(wire, "setBreakpoints").await?;
        assert_eq!(sent["arguments"]["breakpoints"], json!([{"line":3}]));
    }
    let identity = f.editor.debugger().breakpoints(&path)[0].identity();
    wait_for_breakpoint(&mut f, identity).await?;
    assert_eq!(
        f.editor.debugger().get_active_client().unwrap().id(),
        second
    );
    assert!(f.editor.debugger().get_client(first).is_some());
    Ok(())
}
