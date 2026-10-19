use std::time::Duration;

use term::application::Application;
use ui_core::input::parse_macro;

use super::helpers::{run_event_loop_until_idle, AppBuilder};

fn screen(app: &Application) -> String {
    app.terminal_backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

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

async fn wait_for_text(app: &mut Application, expected: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !screen(app).contains(expected) {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(app).await;
        }
    })
    .await
    .map_err(|error| anyhow::anyhow!("waiting for {expected}: {error}; screen: {}", screen(app)))?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn variable_browser_expands_nested_values_and_preserves_parent_navigation(
) -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let id = app.editor.connect_debugger(
        None,
        language_config::DebugAdapterConfig {
            name: "variable fixture".into(),
            transport: language_config::DebugAdapterTransport::Stdio,
            command: env!("CARGO_BIN_EXE_mitos-test-dap").into(),
            args: vec![],
            port_arg: None,
            templates: vec![],
            quirks: Default::default(),
        },
        Some(view::debugger::DebuggerLaunch {
            request: dap::ConnectionType::Launch,
            arguments: serde_json::json!({"inspectVariables":true}),
        }),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(&mut app).await;
            if app
                .editor
                .debugger()
                .get_session(id)
                .is_some_and(|session| session.current_stack_frame().is_some())
            {
                break;
            }
        }
    })
    .await?;
    keys(&mut app, "<space>Gv").await?;
    wait_for_text(&mut app, "globals [expensive]").await?;
    assert!(!screen(&app).contains("answer"));
    keys(&mut app, "locals<ret>").await?;
    wait_for_text(&mut app, "answer").await?;
    keys(&mut app, "object<ret>").await?;
    wait_for_text(&mut app, "nested value").await?;
    keys(&mut app, "back<ret>").await?;
    assert!(screen(&app).contains("object"));
    assert!(!screen(&app).contains("answer")); // The parent's filter is preserved.
    keys(&mut app, "<C-w>back<ret>").await?;
    assert!(screen(&app).contains("locals"));
    keys(&mut app, "<C-w>globals<ret>").await?;
    wait_for_text(&mut app, "true").await?;
    keys(&mut app, "global<ret>").await?;
    assert!(screen(&app).contains("true")); // A leaf keeps the browser open.
    keys(&mut app, "<esc><esc>").await?;
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
