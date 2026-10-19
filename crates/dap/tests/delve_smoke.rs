//! Opt-in real-adapter coverage; the normal suite does not require Go or Delve.

use anyhow::{bail, Context, Result};
use dap::{requests, Client, ClientEvent, DebugAdapterId, Event, Payload, SourceBreakpoint};
use serde_json::json;
use std::{path::Path, time::Duration};
use tokio::{process::Command, sync::mpsc::UnboundedReceiver, time::timeout};

type Incoming = UnboundedReceiver<(DebugAdapterId, ClientEvent)>;

async fn wait_for_event(incoming: &mut Incoming, name: &str) -> Result<Event> {
    timeout(Duration::from_secs(20), async {
        while let Some((_, message)) = incoming.recv().await {
            match message {
                ClientEvent::Message(Payload::Event(event)) => {
                    eprintln!("Delve event: {} {:?}", event.event, event.body);
                    if event.event == name {
                        return Event::parse(&event.event, event.body).map_err(Into::into);
                    }
                }
                other => bail!("expected {name} event, received {other:?}"),
            }
        }
        bail!("adapter closed before {name} event")
    })
    .await
    .with_context(|| format!("timed out waiting for {name} event"))?
}

async fn debug_program(source: &Path, binary: &Path) -> Result<()> {
    let adapter = std::env::var("MITOS_DLV").unwrap_or_else(|_| "dlv".into());
    let (client, mut incoming) = Client::tcp_process(
        &adapter,
        vec!["dap"],
        "--listen=127.0.0.1:{}",
        DebugAdapterId::default(),
    )
    .await?;
    let capabilities = client.initialize("go".into(), false, false).await?;
    assert_eq!(capabilities.supports_configuration_done_request, Some(true));

    let line = include_str!("fixtures/delve/main.go")
        .lines()
        .position(|line| line.contains("// breakpoint"))
        .context("fixture breakpoint marker missing")?
        + 1;
    // Poll launch and configuration together: adapters may defer their launch
    // response until configuration has finished.
    let launch = async {
        client
            .launch(json!({"mode": "exec", "program": binary}))
            .await?;
        Ok::<_, anyhow::Error>(())
    };
    let configure = async {
        wait_for_event(&mut incoming, "initialized").await?;
        let breakpoints = client
            .set_breakpoints(
                source.to_owned(),
                vec![SourceBreakpoint {
                    line,
                    ..Default::default()
                }],
            )
            .await?
            .context("adapter omitted breakpoint verification")?;
        assert_eq!(breakpoints.len(), 1);
        assert!(breakpoints[0].verified, "{breakpoints:?}");
        client.configuration_done().await?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::try_join!(launch, configure)?;

    let Event::Stopped(stopped) = wait_for_event(&mut incoming, "stopped").await? else {
        unreachable!()
    };
    assert_eq!(stopped.reason, "breakpoint");
    let thread = stopped.thread_id.context("stop omitted its thread")?;
    let (frames, _) = client.stack_trace(thread).await?;
    let frame = frames.first().context("missing stopped frame")?;
    assert!(frame.name.ends_with(".add"), "{frame:?}");
    assert_eq!(frame.line, line);
    assert_eq!(
        frame
            .source
            .as_ref()
            .and_then(|source| source.path.as_deref()),
        Some(source)
    );

    let mut value = None;
    for scope in client.scopes(frame.id).await? {
        for variable in client.variables(scope.variables_reference).await? {
            if variable.name == "value" {
                value = Some(variable.value);
            }
        }
    }
    assert_eq!(value.as_deref(), Some("42"));
    assert_eq!(
        client.eval("value".into(), Some(frame.id)).await?.result,
        "42"
    );
    let error = client
        .eval("mitos_missing_variable".into(), Some(frame.id))
        .await
        .unwrap_err();
    let dap::Error::Request(error) = error else {
        bail!("evaluation did not preserve its request failure: {error}")
    };
    assert_eq!(error.command, "evaluate");
    assert!(error.message.is_some() || error.body.is_some());
    client.continue_thread(thread).await?;
    let Event::Exited(exited) = wait_for_event(&mut incoming, "exited").await? else {
        unreachable!()
    };
    assert_eq!(exited.exit_code, 0);
    wait_for_event(&mut incoming, "terminated").await?;
    client
        .disconnect(Some(requests::DisconnectArguments {
            terminate_debuggee: Some(true),
            ..Default::default()
        }))
        .await?;
    timeout(Duration::from_secs(10), async {
        while let Some((_, message)) = incoming.recv().await {
            if let ClientEvent::Closed { error } = message {
                anyhow::ensure!(error.is_none(), "adapter transport failed: {error:?}");
                return Ok(());
            }
        }
        bail!("adapter omitted its closure notification")
    })
    .await
    .context("adapter did not close after disconnect")?
}

#[tokio::test]
#[ignore = "requires Go and Delve; see crates/dap/README.md"]
async fn delve_launch_breakpoint_inspection_and_shutdown() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let source = fixture.path().join("main.go");
    tokio::fs::write(&source, include_str!("fixtures/delve/main.go")).await?;
    // Canonicalize both paths so Darwin's /var -> /private/var alias cannot
    // disagree with the absolute paths embedded by the Go compiler.
    let source = source.canonicalize()?;
    let binary = source.with_file_name(if cfg!(windows) {
        "fixture.exe"
    } else {
        "fixture"
    });
    let output = timeout(
        Duration::from_secs(90),
        Command::new("go")
            .args(["build", "-gcflags=all=-N -l", "-o"])
            .arg(&binary)
            .arg(&source)
            .env("GOWORK", "off")
            .env("GO111MODULE", "off")
            .env("CGO_ENABLED", "0")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("timed out compiling Go fixture")??;
    anyhow::ensure!(
        output.status.success(),
        "Go fixture build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    timeout(Duration::from_secs(90), debug_program(&source, &binary))
        .await
        .context("Delve smoke scenario timed out")?
}
