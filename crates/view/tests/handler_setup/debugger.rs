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
                let body = if message["command"] == "threads" {
                    json!({"threads":[]})
                } else {
                    json!({})
                };
                let response = serde_json::to_vec(&json!({"type":"response", "seq":1,
                    "request_seq":message["seq"], "command":message["command"], "success":!(fail_launch && message["command"] == "launch"), "body":body}))?;
                write
                    .write_all(format!("Content-Length: {}\r\n\r\n", response.len()).as_bytes())
                    .await?;
                write.write_all(&response).await?;
            }
        }
    });
    Ok((socket, receiver))
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
