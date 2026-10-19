use std::time::Duration;

use dap::DebugAdapterId;
use language_config::DebugAdapterConfig;
use term::application::Application;
use tokio::net::TcpListener;
use ui_core::input::parse_macro;

use super::helpers::{run_event_loop_until_idle, test_syntax_loader, AppBuilder};

async fn keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    for key in parse_macro(keys)? {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(key.into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(key.into());
        app.handle_terminal_events(Ok(event)).await;
    }
    app.editor.reset_idle_timer();
    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(app)).await?;
    Ok(())
}

fn screen(app: &Application) -> String {
    app.terminal_backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn connect(app: &mut Application, listener: &TcpListener, name: &str) -> DebugAdapterId {
    // Keep the adapter pending so the test controls its lifetime through the UI.
    app.editor.connect_debugger(
        Some(listener.local_addr().unwrap()),
        DebugAdapterConfig {
            name: name.into(),
            transport: language_config::DebugAdapterTransport::Tcp,
            command: String::new(),
            args: vec![],
            port_arg: None,
            templates: vec![],
            quirks: Default::default(),
        },
        None,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn session_picker_selects_pending_sessions_and_termination_restores_the_previous_one(
) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let mut app = AppBuilder::new().build()?;
    let backend = connect(&mut app, &listener, "backend");
    let frontend = connect(&mut app, &listener, "frontend");
    keys(&mut app, "<space>Gss").await?;
    let text = screen(&app);
    assert!(text.contains("#1 backend"));
    assert!(text.contains("#2 frontend"));
    assert!(text.contains("starting"));
    keys(&mut app, "backend<ret>").await?;
    assert_eq!(
        app.editor.debugger().get_active_session().unwrap().id(),
        backend
    );
    let text = screen(&app);
    assert!(text.contains("Debug #1 backend (starting, 2 sessions)"));
    // The debug menu is sticky; escape it before starting a new key sequence.
    keys(&mut app, "<esc><space>Gc").await?;
    assert!(screen(&app).contains("Selected debugging session is still starting"));
    keys(&mut app, "<esc><space>Gt").await?;
    assert!(app.editor.debugger().get_session(backend).is_none());
    assert_eq!(
        app.editor.debugger().get_active_session().unwrap().id(),
        frontend
    );
    assert!(screen(&app).contains("Debug #2 frontend (starting, 1 session)"));
    keys(&mut app, "<esc><space>Gt<esc>").await?;
    assert!(app.editor.debugger().is_empty());
    assert!(!screen(&app).contains("Debug #"));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_menu_allows_existing_sessions_and_picker_rejects_ended_ones() -> anyhow::Result<()>
{
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("main.debug-test");
    std::fs::write(&path, "text\n")?;
    let languages = r#"
        [[language]]
        name = "debug-test"
        scope = "source.debug-test"
        file-types = ["debug-test"]
        [language.debugger]
        name = "fixture"
        transport = "stdio"
        command = "unused-debug-adapter"
        [[language.debugger.templates]]
        name = "Launch another target"
        request = "launch"
        args = {}
    "#;
    let mut app = AppBuilder::new()
        .with_file(path, None)
        .with_lang_loader(test_syntax_loader(Some(languages.into())))
        .build()?;
    let original = connect(&mut app, &listener, "original");
    keys(&mut app, "<space>Gl").await?;
    assert!(screen(&app).contains("Launch another target"));
    keys(&mut app, "<esc><esc><space>Gss").await?;
    // Remove the row's session while its picker remains open, then reuse the slot.
    assert!(app.editor.debugger_mut().cancel_startup(original));
    let replacement = connect(&mut app, &listener, "replacement");
    assert_ne!(original, replacement);
    keys(&mut app, "<ret>").await?;
    assert_eq!(
        app.editor.debugger().get_active_session().unwrap().id(),
        replacement
    );
    assert_eq!(app.editor.debugger().sessions().len(), 1);
    assert!(screen(&app).contains("Debugging session has ended"));
    keys(&mut app, "<esc><space>Gss<esc><esc>").await?;
    assert_eq!(
        app.editor.debugger().get_active_session().unwrap().id(),
        replacement
    );
    app.editor.debugger_mut().cancel_startup(replacement);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_adapters_can_start_child_sessions_without_changing_selection() -> anyhow::Result<()>
{
    let mut app = AppBuilder::new().build()?;
    let parent = app.editor.connect_debugger(
        None,
        DebugAdapterConfig {
            name: "stdio fixture".into(),
            transport: language_config::DebugAdapterTransport::Stdio,
            command: env!("CARGO_BIN_EXE_mitos-test-dap").into(),
            args: vec![],
            port_arg: None,
            templates: vec![],
            quirks: Default::default(),
        },
        Some(view::debugger::DebuggerLaunch {
            request: dap::ConnectionType::Launch,
            arguments: serde_json::json!({"spawnChild":true}),
        }),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(&mut app).await;
            if !app.editor.debugger().is_starting(parent)
                && app.editor.debugger().sessions().any(|session| {
                    session.parent() == Some(parent)
                        && !app.editor.debugger().is_starting(session.id())
                })
            {
                break;
            }
        }
    })
    .await?;
    let child = app
        .editor
        .debugger()
        .sessions()
        .find(|session| session.parent() == Some(parent))
        .unwrap()
        .id();
    assert!(!app.editor.debugger().is_starting(child));
    assert_eq!(app.editor.debugger().sessions().len(), 2);
    assert_eq!(
        app.editor.debugger().get_active_session().unwrap().id(),
        parent
    );
    for id in [child, parent] {
        app.editor.select_debugger_session(id)?;
        app.editor
            .debugger_command(view::debugger::DebuggerCommand::Terminate)?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while app.editor.debugger().get_session(id).is_some() {
                run_event_loop_until_idle(&mut app).await;
            }
        })
        .await?;
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

fn managed_tcp(app: &mut Application, args: Vec<String>) -> DebugAdapterId {
    app.editor.connect_debugger(
        None,
        DebugAdapterConfig {
            name: "TCP fixture".into(),
            transport: language_config::DebugAdapterTransport::Tcp,
            command: env!("CARGO_BIN_EXE_mitos-test-dap").into(),
            args,
            port_arg: Some("--tcp {}".into()),
            templates: vec![],
            quirks: Default::default(),
        },
        None,
    )
}

async fn wait_for_probe(path: &std::path::Path) -> anyhow::Result<std::net::SocketAddr> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match std::fs::read_to_string(path) {
                Ok(address) => return Ok(address.parse()?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => return Err(error.into()),
            }
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn managed_tcp_startup_waits_for_adapter_readiness() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = dir.path().join("ready");
    let started = dir.path().join("started");
    let mut app = AppBuilder::new().build()?;
    let id = managed_tcp(
        &mut app,
        vec![
            "--gate".into(),
            gate.to_string_lossy().into(),
            "--started".into(),
            started.to_string_lossy().into(),
        ],
    );
    wait_for_probe(&started).await?;
    assert!(app.editor.debugger().is_starting(id));
    std::fs::write(gate, "ready")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.editor.debugger().is_starting(id) {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    assert!(app
        .editor
        .debugger()
        .get_session(id)
        .unwrap()
        .is_connected());
    app.editor
        .debugger_command(view::debugger::DebuggerCommand::Terminate)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.editor.debugger().get_session(id).is_some() {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn managed_tcp_startup_reports_early_process_exit() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let id = managed_tcp(&mut app, vec!["--exit".into()]);
    tokio::time::timeout(Duration::from_secs(5), async {
        while app.editor.debugger().get_session(id).is_some() {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    assert!(app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("exited before accepting connections"));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canceling_tcp_startup_releases_the_adapter_process() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let started = dir.path().join("started");
    let mut app = AppBuilder::new().build()?;
    let id = managed_tcp(
        &mut app,
        vec![
            "--gate".into(),
            dir.path().join("never-ready").to_string_lossy().into(),
            "--started".into(),
            started.to_string_lossy().into(),
        ],
    );
    let probe = wait_for_probe(&started).await?;
    assert!(app.editor.debugger_mut().cancel_startup(id));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match TcpListener::bind(probe).await {
                Ok(listener) => {
                    drop(listener);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::task::yield_now().await
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    })
    .await??;
    assert!(app.editor.debugger().get_session(id).is_none());
    assert!(app.close().await.is_empty());
    Ok(())
}
