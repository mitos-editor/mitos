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

async fn wait_for_row(app: &mut Application, name: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        // One occurrence is the query; another must be the actual matched row.
        while screen(app).matches(name).count() < 2 {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(app).await;
        }
    })
    .await?;
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
    // A refresh must retain the nested page and its filter, then retain parent filters too.
    keys(&mut app, "child").await?;
    app.editor
        .debugger_evaluate("update".into())?
        .run()
        .await
        .unwrap()
        .into_result(&app.editor)
        .unwrap()?;
    wait_for_text(&mut app, "updated value").await?;
    assert!(screen(&app).contains("child"));
    keys(&mut app, "<C-w>").await?;
    keys(&mut app, "back<ret>").await?;
    assert!(screen(&app).contains("object"));
    assert!(!screen(&app).contains("answer")); // The parent's filter is preserved.
    keys(&mut app, "<C-w>back<ret>").await?;
    assert!(screen(&app).contains("locals"));
    keys(&mut app, "<C-w>globals<ret>").await?;
    wait_for_text(&mut app, "true").await?;
    keys(&mut app, "global<ret>").await?;
    assert!(screen(&app).contains("true")); // A leaf keeps the browser open.
    keys(&mut app, "<C-w>back<ret><C-w>large<ret>").await?;
    wait_for_text(&mut app, "Load more").await?;
    keys(&mut app, "Load more<ret>").await?;
    keys(&mut app, "<C-w><C-w>item199").await?;
    wait_for_row(&mut app, "item199").await?;
    assert!(!screen(&app).contains("item200"));
    keys(&mut app, "<C-w>Load more<ret><C-w><C-w>item249").await?;
    wait_for_row(&mut app, "item249").await?;
    keys(&mut app, "<C-w>").await?;
    assert!(!screen(&app).contains("Load more"));
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

#[tokio::test(flavor = "multi_thread")]
async fn frame_picker_loads_deep_frames_and_preserves_its_filter() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let id = app.editor.connect_debugger(
        None,
        language_config::DebugAdapterConfig {
            name: "stack fixture".into(),
            transport: language_config::DebugAdapterTransport::Stdio,
            command: env!("CARGO_BIN_EXE_mitos-test-dap").into(),
            args: vec![],
            port_arg: None,
            templates: vec![],
            quirks: Default::default(),
        },
        Some(view::debugger::DebuggerLaunch {
            request: dap::ConnectionType::Launch,
            arguments: serde_json::json!({"inspectStack":true}),
        }),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
            if app
                .editor
                .debugger()
                .get_session(id)
                .is_some_and(|s| s.current_stack_frame().is_some())
            {
                break;
            }
        }
    })
    .await?;
    keys(&mut app, "<space>Gsf").await?;
    wait_for_text(&mut app, "Load more").await?;
    for expected in [64, 70] {
        keys(
            &mut app,
            if expected == 64 {
                "Load more<ret>"
            } else {
                "<ret>"
            },
        )
        .await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                app.editor.reset_idle_timer();
                run_event_loop_until_idle(&mut app).await;
                let session = app.editor.debugger().get_session(id).unwrap();
                if session
                    .stack_frames()
                    .get(&session.selected_thread().unwrap())
                    .unwrap()
                    .frames()
                    .len()
                    == expected
                {
                    break;
                }
            }
        })
        .await?;
    }
    keys(&mut app, "<C-w><C-w>frame69").await?;
    wait_for_text(&mut app, "frame69 (breakpoint)").await?;
    keys(&mut app, "<ret>").await?;
    assert_eq!(
        app.editor
            .debugger()
            .get_session(id)
            .unwrap()
            .current_stack_frame()
            .unwrap()
            .id,
        70
    );
    app.editor
        .debugger_command(view::debugger::DebuggerCommand::Terminate)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.editor.debugger().get_session(id).is_some() {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_source_navigation_opens_an_immutable_named_document() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let id = app.editor.connect_debugger(
        None,
        language_config::DebugAdapterConfig {
            name: "source fixture".into(),
            transport: language_config::DebugAdapterTransport::Stdio,
            command: env!("CARGO_BIN_EXE_mitos-test-dap").into(),
            args: vec![],
            port_arg: None,
            templates: vec![],
            quirks: Default::default(),
        },
        Some(view::debugger::DebuggerLaunch {
            request: dap::ConnectionType::Launch,
            arguments: serde_json::json!({"inspectSource":true}),
        }),
    );
    wait_for_text(&mut app, "adapter content").await?;
    wait_for_text(&mut app, "generated.rs").await?;
    let document = view::current_ref!(app.editor).1;
    assert!(document.is_readonly());
    assert!(document.path().is_none());
    assert!(document.display_name().contains("generated.rs"));
    keys(&mut app, "ix<esc>").await?;
    assert_eq!(
        view::current_ref!(app.editor).1.text().to_string(),
        "adapter content\n"
    );
    app.editor
        .debugger_command(view::debugger::DebuggerCommand::Terminate)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.editor.debugger().get_session(id).is_some() {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    assert!(app.close().await.is_empty());
    Ok(())
}
