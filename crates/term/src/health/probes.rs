//! Bounded initialization probes using the editor's protocol clients.

use super::output::{Output, Status};

use language_config::{
    DebugAdapterConfig, DebugAdapterTransport, LanguageConfiguration, LanguageServerConfiguration,
    LanguageServerFeature, LanguageServerFeatures,
};
use lsp_client::{self as lsp, jsonrpc, lsp as protocol, Call};
use serde_json::{json, Value};
use std::{collections::HashMap, io::Write, time::Duration};
use tokio::time::timeout;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(200);

pub(super) async fn language_servers(
    output: &mut Output<impl Write>,
    language: &LanguageConfiguration,
    definitions: &HashMap<String, LanguageServerConfiguration>,
    config: &editor_config::LspConfig,
) -> std::io::Result<()> {
    if language.language_servers.is_empty() {
        return Ok(());
    }
    writeln!(output)?;
    output.heading("LSP capabilities (initialize; up to 5s per server)")?;
    output.note("Probes include alternatives; this does not select the active server.")?;
    if !config.enable {
        output.row(
            Status::Inactive,
            "integration",
            "Editor LSP integration is disabled (editor.lsp.enable = false).",
        )?;
    }
    let (root, uri) =
        view::language_servers::language_server_root(language, None, &config.workspace_roots);
    for server in &language.language_servers {
        writeln!(output)?;
        output.heading(&format!("  {}", server.name))?;
        let Some(definition) = definitions.get(&server.name) else {
            output.row(
                Status::Unknown,
                "capabilities",
                "unknown: definition missing",
            )?;
            continue;
        };
        if stdx::env::which(&definition.command).is_err() {
            output.row(
                Status::Unknown,
                "capabilities",
                "unknown: executable missing",
            )?;
            continue;
        }
        match view::language_servers::has_required_root(definition, &root) {
            Ok(true) => {}
            Ok(false) => {
                output.row(
                    Status::Inactive,
                    "capabilities",
                    "Skipped: project does not match required-root-patterns",
                )?;
                continue;
            }
            Err(error) => {
                output.row(
                    Status::Unknown,
                    "capabilities",
                    format_args!("unknown: {error}"),
                )?;
                continue;
            }
        }
        output.flush()?;
        match timeout(
            PROBE_TIMEOUT,
            initialize_lsp(
                definition,
                server,
                root.clone(),
                uri.clone(),
                config.completion.snippets,
            ),
        )
        .await
        {
            Ok(Ok(capabilities)) => lsp_capabilities(output, &capabilities, server)?,
            Ok(Err(error)) => output.row(
                Status::Unknown,
                "capabilities",
                format_args!("Initialization failed; capabilities unknown: {error}"),
            )?,
            Err(_) => output.row(
                Status::Unknown,
                "capabilities",
                "Initialization timed out after 5s; capabilities unknown",
            )?,
        }
    }
    Ok(())
}

async fn initialize_lsp(
    definition: &LanguageServerConfiguration,
    server: &LanguageServerFeatures,
    root: std::path::PathBuf,
    uri: Option<protocol::Url>,
    snippets: bool,
) -> anyhow::Result<protocol::ServerCapabilities> {
    let (client, mut incoming) = lsp::Client::start(
        &definition.command,
        &definition.args,
        definition.config.clone(),
        &definition.environment,
        root,
        uri,
        lsp::LanguageServerId::default(),
        server.name.clone(),
        definition.timeout,
    )?;
    let initialize = client.initialize(
        snippets,
        protocol::ClientInfo {
            name: "mitos".to_owned(),
            version: Some(loader::VERSION_AND_GIT_HASH.to_owned()),
        },
        protocol::TraceValue::Off,
    );
    tokio::pin!(initialize);
    loop {
        tokio::select! {
            result = &mut initialize => { result?; break; }
            call = incoming.recv() => match call {
                Some((_, Call::MethodCall(call))) => reply_lsp(&client, call).await?,
                Some(_) => {},
                None => anyhow::bail!("server closed the stream"),
            }
        }
    }
    let capabilities = client.capabilities().clone();
    // A health check never opens a document or applies workspace edits.
    let _ = timeout(SHUTDOWN_TIMEOUT, client.shutdown()).await;
    client.exit();
    let _ = timeout(SHUTDOWN_TIMEOUT, client.wait_shutdown_flushed()).await;
    Ok(capabilities)
}

async fn reply_lsp(client: &lsp::Client, call: jsonrpc::MethodCall) -> lsp::Result<()> {
    let result = match call.method.as_str() {
        "workspace/configuration" => call
            .params
            .parse::<protocol::ConfigurationParams>()
            .and_then(|params| {
                serde_json::to_value(client.configuration(&params))
                    .map_err(|_| jsonrpc::Error::invalid_params("Invalid configuration request"))
            }),
        "workspace/workspaceFolders" => Ok(json!(client.workspace_folders().as_slice())),
        "window/workDoneProgress/create" | "window/showMessageRequest" => Ok(Value::Null),
        _ => Err(jsonrpc::Error {
            code: jsonrpc::ErrorCode::MethodNotFound,
            message: "Request unavailable during health check".to_owned(),
            data: None,
        }),
    };
    client.reply(call.id, result)
}

fn lsp_capabilities(
    output: &mut Output<impl Write>,
    capabilities: &protocol::ServerCapabilities,
    server: &LanguageServerFeatures,
) -> std::io::Result<()> {
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();
    let mut disabled = Vec::new();
    for &feature in LanguageServerFeature::all() {
        let label = feature.to_string();
        if !server.has_feature(feature) {
            disabled.push(label);
        } else if feature == LanguageServerFeature::Diagnostics {
            // publishDiagnostics has no server capability; initialization cannot prove support.
        } else if view::language_servers::supports_feature(capabilities, feature) {
            supported.push(label);
        } else {
            unsupported.push(label);
        }
    }
    feature_list(output, Status::Ok, "Supported", &supported)?;
    feature_list(output, Status::Inactive, "Not advertised", &unsupported)?;
    feature_list(
        output,
        Status::Inactive,
        "Disabled by configuration",
        &disabled,
    )?;
    if server.has_feature(LanguageServerFeature::Diagnostics) {
        output.row(
            Status::Unknown,
            "diagnostics",
            "Push diagnostics: not advertised by LSP; requires an open document",
        )?;
    }
    Ok(())
}

pub(super) async fn debug_adapter(
    output: &mut Output<impl Write>,
    config: Option<&DebugAdapterConfig>,
) -> std::io::Result<()> {
    let Some(config) = config else {
        output.heading("Debug adapter")?;
        return output.row(Status::Inactive, "adapter", "not configured");
    };
    let transport = match config.transport {
        DebugAdapterTransport::Stdio => "stdio",
        DebugAdapterTransport::Tcp => "tcp",
    };
    super::probe_protocol(
        output,
        &format!("debug adapter {} ({transport})", config.name),
        Some((&config.command, &config.args)),
    )?;
    if stdx::env::which(&config.command).is_err() {
        return output.row(
            Status::Unknown,
            "capabilities",
            "unknown: executable missing",
        );
    }
    output.detail("Capabilities (initialize; up to 5s)")?;
    output.flush()?;
    match timeout(PROBE_TIMEOUT, initialize_dap(config)).await {
        Ok(Ok(capabilities)) => dap_capabilities(output, &capabilities),
        Ok(Err(error)) => output.row(
            Status::Unknown,
            "capabilities",
            format_args!("Initialization failed; capabilities unknown: {error}"),
        ),
        Err(_) => output.row(
            Status::Unknown,
            "capabilities",
            "Initialization timed out after 5s; capabilities unknown",
        ),
    }
}

async fn initialize_dap(config: &DebugAdapterConfig) -> anyhow::Result<dap::DebuggerCapabilities> {
    let args = config.args.iter().map(String::as_str).collect();
    let id = dap::DebugAdapterId::default();
    let (client, mut incoming) = match config.transport {
        DebugAdapterTransport::Stdio => dap::Client::stdio(&config.command, args, id)?,
        DebugAdapterTransport::Tcp => {
            dap::Client::tcp_process(
                &config.command,
                args,
                config
                    .port_arg
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("TCP adapter requires port-arg"))?,
                id,
            )
            .await?
        }
    };
    // No launch/attach or terminal/debuggee processes are needed for capabilities.
    let initialize = client.initialize(config.name.clone(), false, false);
    tokio::pin!(initialize);
    let capabilities = loop {
        tokio::select! {
            result = &mut initialize => break result?,
            event = incoming.recv() => match event {
                Some((_, dap::ClientEvent::Message(dap::Payload::Request(request)))) => {
                    client.reply(request.seq, &request.command, Err(dap::Error::Unhandled))?;
                }
                Some((_, dap::ClientEvent::Closed { .. })) | None => anyhow::bail!("adapter closed the stream"),
                _ => {},
            }
        }
    };
    let _ = timeout(SHUTDOWN_TIMEOUT, client.disconnect(None)).await;
    Ok(capabilities)
}

fn dap_capabilities(
    output: &mut Output<impl Write>,
    capabilities: &dap::DebuggerCapabilities,
) -> std::io::Result<()> {
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();
    macro_rules! features {
        ($($field:ident => $label:literal),* $(,)?) => {
            $(if capabilities.$field == Some(true) { supported.push($label.to_owned()); }
                else { unsupported.push($label.to_owned()); })*
        };
    }
    features! {
        supports_configuration_done_request => "configuration-done",
        supports_function_breakpoints => "function-breakpoints",
        supports_conditional_breakpoints => "conditional-breakpoints",
        supports_hit_conditional_breakpoints => "hit-conditional-breakpoints",
        supports_evaluate_for_hovers => "evaluate-for-hovers",
        supports_step_back => "step-back",
        supports_set_variable => "set-variable",
        supports_restart_frame => "restart-frame",
        supports_goto_targets_request => "goto-targets",
        supports_step_in_targets_request => "step-in-targets",
        supports_completions_request => "completions",
        supports_modules_request => "modules",
        supports_restart_request => "restart",
        supports_exception_options => "exception-options",
        supports_value_formatting_options => "value-formatting",
        supports_exception_info_request => "exception-info",
        support_terminate_debuggee => "terminate-debuggee",
        support_suspend_debuggee => "suspend-debuggee",
        supports_delayed_stack_trace_loading => "delayed-stack-trace-loading",
        supports_loaded_sources_request => "loaded-sources",
        supports_log_points => "log-points",
        supports_terminate_threads_request => "terminate-threads",
        supports_set_expression => "set-expression",
        supports_terminate_request => "terminate",
        supports_data_breakpoints => "data-breakpoints",
        supports_read_memory_request => "read-memory",
        supports_write_memory_request => "write-memory",
        supports_disassemble_request => "disassemble",
        supports_cancel_request => "cancel",
        supports_breakpoint_locations_request => "breakpoint-locations",
        supports_clipboard_context => "clipboard-context",
        supports_stepping_granularity => "stepping-granularity",
        supports_instruction_breakpoints => "instruction-breakpoints",
        supports_exception_filter_options => "exception-filter-options",
    }
    output.note("Adapter-advertised features; Mitos may expose only a subset.")?;
    feature_list(output, Status::Ok, "Supported", &supported)?;
    feature_list(output, Status::Inactive, "Not advertised", &unsupported)
}

fn feature_list(
    output: &mut Output<impl Write>,
    status: Status,
    label: &str,
    features: &[String],
) -> std::io::Result<()> {
    output.feature_list(status, label, features, super::terminal_columns())
}
