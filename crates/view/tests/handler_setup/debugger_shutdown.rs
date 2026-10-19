use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_rejects_launches_and_cancels_queued_terminal_completion() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let calls = Arc::new(Mutex::new(Vec::new()));
    f.editor.debugger_mut().set_terminal_launcher(Launcher {
        calls: calls.clone(),
        fail: false,
        gate: None,
    })?;
    let (id, mut wire) = adapter_with_options(
        &mut f,
        AdapterOptions {
            held_commands: vec!["terminate"],
            failed_commands: vec!["terminate"],
            ..Default::default()
        },
    )
    .await?;
    command(&mut wire, "initialize").await?;
    f.editor
        .handle_debugger_message(
            id,
            request("runInTerminal", 60, json!({"cwd":"", "args":["program"]})),
        )
        .await;
    let queued = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .unwrap();
    f.editor
        .debugger_command(view::debugger::DebuggerCommand::Terminate)?;
    let mut cancellation = None;
    let mut terminate = false;
    while cancellation.is_none() || !terminate {
        let message = message(&mut wire).await?;
        if message["type"] == "response" {
            cancellation = Some(message);
        } else {
            assert_eq!(message["command"], "terminate");
            terminate = true;
        }
    }
    assert_eq!(cancellation.as_ref().unwrap()["request_seq"], 60);
    assert_eq!(cancellation.unwrap()["success"], false);
    queued(&mut f.editor);
    for (sequence, name, arguments) in [
        (61, "runInTerminal", json!({"cwd":"", "args":["late"]})),
        (
            62,
            "startDebugging",
            json!({"request":"launch", "configuration":{}}),
        ),
    ] {
        f.editor
            .handle_debugger_message(id, request(name, sequence, arguments))
            .await;
        let reply = message(&mut wire).await?;
        assert_eq!(reply["request_seq"], sequence);
        assert_eq!(reply["success"], false);
    }
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(f.editor.debugger().sessions().len(), 1);
    assert!(wire.try_recv().is_err());
    release_adapter_request(&wire).await?;
    debugger_callback(&mut f).await?;
    // Rejected shutdown permits a fresh request; the canceled one cannot reply again.
    f.editor
        .handle_debugger_message(
            id,
            request("runInTerminal", 63, json!({"cwd":"", "args":["retry"]})),
        )
        .await;
    debugger_callback(&mut f).await?;
    let reply = message(&mut wire).await?;
    assert_eq!(reply["request_seq"], 63);
    assert_eq!(reply["success"], true);
    assert_eq!(calls.lock().unwrap().len(), 2);
    Ok(())
}
