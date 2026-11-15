//! Trace negotiation and reload use real server transport; diagnostics stay local.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use editor_config::LspTrace;
use lsp_client::{jsonrpc, Call, LanguageServerId};
use serde_json::{json, Value};
use term::application::TestApplication;

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config,
};

fn application(
    dir: &Path,
    trace: LspTrace,
    notifications: bool,
) -> anyhow::Result<(TestApplication, Gate, PathBuf)> {
    let gate = Gate::new(dir.join("initialize-ready"));
    let log = dir.join("wire.requests");
    let mut server = ServerConfig::feature("alpha", "--language-features", dir)
        .initialize_gate(&gate)
        .arg("--request-log")
        .arg(&log);
    if notifications {
        server = server.arg("--trace-events");
    }
    let mut config = test_config();
    config.editor.lsp.trace = trace;
    let (app, _) = lsp::feature_app_with_servers(
        dir,
        "😀 source\n",
        "trace-test",
        &["alpha"],
        config,
        &server.toml(),
        |_, _, _| {},
    )?;
    Ok((app, gate, log))
}

fn reload(app: &mut TestApplication, trace: LspTrace) {
    let old = (*app.editor.config()).clone();
    let mut new = old.clone();
    new.lsp.trace = trace;
    app.editor.config = Arc::new(arc_swap::ArcSwap::from_pointee(new));
    app.editor.refresh_config(&old);
}

async fn traces(log: &Path, expected: usize) -> anyhow::Result<Vec<Value>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let traces: Vec<Value> = lsp::read_log(log)?
                .into_iter()
                .filter(|message| message["method"] == "$/setTrace")
                .map(|message| message["params"]["value"].clone())
                .collect();
            if traces.len() >= expected {
                return Ok(traces);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn initialization_negotiates_each_trace_setting_before_opening_documents(
) -> anyhow::Result<()> {
    for (trace, name) in [
        (LspTrace::Off, "off"),
        (LspTrace::Messages, "messages"),
        (LspTrace::Verbose, "verbose"),
    ] {
        let dir = tempfile::tempdir()?;
        let (mut app, gate, log) = application(dir.path(), trace, false)?;
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        lsp::wait_message(&log, "textDocument/didOpen").await?;
        let messages: Vec<Value> = lsp::read_log(&log)?;
        let initialize = messages
            .iter()
            .find(|message| message["method"] == "initialize")
            .unwrap();
        assert_eq!(initialize["params"]["trace"], name);
        assert_eq!(traces(&log, 1).await?, [json!(name)]);
        let index = |method| {
            messages
                .iter()
                .position(|message| message["method"] == method)
                .unwrap()
        };
        assert!(index("initialized") < index("$/setTrace"));
        assert!(index("$/setTrace") < index("textDocument/didOpen"));
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_updates_trace_including_off_without_restarting_the_server() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, gate, log) = application(dir.path(), LspTrace::Off, false)?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    assert_eq!(traces(&log, 1).await?, [json!("off")]);
    for (index, trace) in [LspTrace::Messages, LspTrace::Verbose, LspTrace::Off]
        .into_iter()
        .enumerate()
    {
        reload(&mut app, trace);
        traces(&log, index + 2).await?;
    }
    assert_eq!(
        traces(&log, 4).await?,
        [
            json!("off"),
            json!("messages"),
            json!("verbose"),
            json!("off")
        ]
    );
    let messages: Vec<Value> = lsp::read_log(&log)?;
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "initialize")
            .count(),
        1
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_while_initializing_applies_the_latest_trace_before_document_open(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, gate, log) = application(dir.path(), LspTrace::Messages, false)?;
    let initialize = lsp::wait_message(&log, "initialize").await?;
    assert_eq!(initialize["params"]["trace"], "messages");
    reload(&mut app, LspTrace::Verbose);
    let messages: Vec<Value> = lsp::read_log(&log)?;
    assert!(!messages
        .iter()
        .any(|message| message["method"] == "$/setTrace"));
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    assert_eq!(traces(&log, 1).await?, [json!("verbose")]);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn server_trace_telemetry_and_malformed_payloads_do_not_replace_status() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let (mut app, gate, _) = application(dir.path(), LspTrace::Off, true)?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    app.editor.set_status("keep this status");
    for _ in 0..6 {
        let (server, call) = tokio::time::timeout(
            Duration::from_secs(10),
            app.editor.next_language_server_message(),
        )
        .await?
        .unwrap();
        app.handle_language_server_message(call, server).await;
        assert_eq!(
            app.editor.get_status().map(|(message, _)| message.as_ref()),
            Some("keep this status")
        );
    }
    for method in ["$/logTrace", "telemetry/event"] {
        let call = Call::Notification(jsonrpc::Notification {
            jsonrpc: Some(jsonrpc::Version::V2),
            method: method.into(),
            params: json!({"message": "removed server"}).into(),
        });
        app.handle_language_server_message(call, LanguageServerId::default())
            .await;
        assert_eq!(
            app.editor.get_status().map(|(message, _)| message.as_ref()),
            Some("keep this status")
        );
    }
    assert!(app.close().await.is_empty());
    Ok(())
}
