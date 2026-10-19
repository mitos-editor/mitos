use std::time::Duration;

use anyhow::Context as _;
use serde_json::json;
use view::{current_ref, editor::Action};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    run_event_loop_until_idle, test_config, AppBuilder,
};

#[tokio::test(flavor = "multi_thread")]
async fn inline_debug_values_combine_lsp_ranges_with_dap_values_and_support_server_refresh(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("ready"));
    let server = ServerConfig::feature("inline", "--inline-values", dir.path())
        .initialize_gate(&gate)
        .toml();
    let loader = lsp::syntax_loader("inline-test", "inline-test", &["inline"], &server);
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(loader)
        .build()?;
    let path = dir.path().join("document.inline-test");
    std::fs::write(&path, "😀 answer\n")?;
    app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    let server = current_ref!(app.editor)
        .1
        .language_servers()
        .next()
        .context("missing server")?
        .id();
    let id = app.editor.connect_debugger(
        None,
        language_config::DebugAdapterConfig {
            name: "inline values".into(),
            transport: language_config::DebugAdapterTransport::Stdio,
            command: env!("CARGO_BIN_EXE_mitos-test-dap").into(),
            args: vec![],
            port_arg: None,
            templates: vec![],
            quirks: Default::default(),
        },
        Some(view::debugger::DebuggerLaunch {
            request: dap::ConnectionType::Launch,
            arguments: json!({"inspectVariables":true, "sourcePath":path}),
        }),
    );
    let expected = "runtime · answer = 42 · ANSWER = 42 · answer + 1 = 43";
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
            let view = current_ref!(app.editor).0.id;
            if app
                .editor
                .debugger_inline_values(view)
                .first()
                .is_some_and(|value| value.text == expected)
            {
                break;
            }
        }
    })
    .await
    .context("initial inline values did not arrive")?;
    let screen: String = app
        .terminal_backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains(expected), "{screen}");
    let requests = lsp::read_log(&lsp::log_path(dir.path(), "inline"))?;
    let request = requests
        .iter()
        .find(|message| message["method"] == "textDocument/inlineValue")
        .unwrap();
    assert_eq!(request["params"]["context"]["frameId"], 1);
    assert_eq!(
        request["params"]["context"]["stoppedLocation"]["start"]["line"],
        0
    );
    let capabilities = &requests
        .iter()
        .find(|message| message["method"] == "initialize")
        .unwrap()["params"]["capabilities"];
    assert_eq!(
        capabilities["textDocument"]["inlineValue"]["dynamicRegistration"],
        false
    );
    assert_eq!(
        capabilities["workspace"]["inlineValue"]["refreshSupport"],
        true
    );
    app.handle_language_server_message(lsp_client::Call::MethodCall(serde_json::from_value(json!({
        "jsonrpc":"2.0", "id":"inline-refresh", "method":"workspace/inlineValue/refresh", "params":null
    }))?), server).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
            let messages = lsp::read_log(&lsp::log_path(dir.path(), "inline")).unwrap();
            if messages
                .iter()
                .filter(|message| message["method"] == "textDocument/inlineValue")
                .count()
                == 2
                && !app
                    .editor
                    .debugger_inline_values(current_ref!(app.editor).0.id)
                    .is_empty()
            {
                break;
            }
        }
    })
    .await
    .context("server refresh did not complete")?;
    app.editor
        .handle_debugger_message(
            id,
            serde_json::from_value(json!({
                "type":"event", "seq":0, "event":"capabilities",
                "body":{"capabilities":{"supportsEvaluateForHovers":false}}
            }))?,
        )
        .await;
    // Direct editor calls bypass the application's normal debugger-event redraw.
    app.editor.redraw().request();
    let without_expression = "runtime · answer = 42 · ANSWER = 42";
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
            if app
                .editor
                .debugger_inline_values(current_ref!(app.editor).0.id)
                .first()
                .is_some_and(|value| value.text == without_expression)
            {
                break;
            }
        }
    })
    .await
    .context("inline values did not follow updated adapter capabilities")?;
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
