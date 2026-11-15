//! Minimal stdio language server for document-feature integration tests.
//! Full text sync and UTF-16 positions exercise the real client transport.
use std::{
    collections::HashMap,
    io::{self, BufRead, Read, Write},
};

use serde_json::{json, Value};

fn respond(output: &mut impl Write, message: Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(&message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

fn respond_result(output: &mut impl Write, id: &Value, result: Value) -> anyhow::Result<()> {
    respond(
        output,
        json!({"jsonrpc": "2.0", "id": id, "result": result}),
    )
}

fn wait_for_gate(args: &[String], argument: &str) {
    if let Some(index) = args.iter().position(|arg| arg == argument) {
        while !std::path::Path::new(&args[index + 1]).exists() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn diagnostic_reports(path: &str) -> anyhow::Result<Value> {
    let file = std::fs::File::open(path)?;
    file.lock_shared()?;
    Ok(serde_json::from_reader(file)?)
}

fn range(start: usize, end: usize) -> Value {
    json!({"start": {"line": 0, "character": start}, "end": {"line": 0, "character": end}})
}

fn main() -> anyhow::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut documents = HashMap::<String, String>::new();
    let mut versions = HashMap::<String, i64>::new();
    let mut diagnostic_requests = HashMap::<(String, i64), usize>::new();
    let mut inlay_refresh_sent = false;
    // Optional modes expose only the feature under test.
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--echo") {
        io::copy(&mut input, &mut output)?;
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--format") {
        // Consume the document before exiting so the editor never hits a broken pipe.
        io::copy(&mut input, &mut io::sink())?;
        output.write_all(b"new content\n")?;
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--hang") {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    let file_operations = args.first().is_some_and(|arg| arg == "--file-operations");
    let rename = args.first().is_some_and(|arg| arg == "--rename");
    let workspace_symbols = args.first().is_some_and(|arg| arg == "--workspace-symbols");
    let document_symbols = args.first().is_some_and(|arg| arg == "--document-symbols");
    let formatting = args.first().is_some_and(|arg| arg == "--formatting");
    let editing_hooks = args.first().is_some_and(|arg| arg == "--editing-hooks");
    let diagnostics = args.first().is_some_and(|arg| arg == "--diagnostics");
    let action_execution = args.first().is_some_and(|arg| arg == "--action-execution");
    let code_actions = args.first().is_some_and(|arg| arg == "--code-actions");
    let structural_ranges = args.first().is_some_and(|arg| arg == "--structural-ranges");
    let linked_editing = args.first().is_some_and(|arg| arg == "--linked-editing");
    let mut folding_requests = 0;
    let semantic_tokens = args.first().is_some_and(|arg| arg == "--semantic-tokens");
    let semantic_deltas = args.iter().any(|arg| arg == "--deltas");
    let mut semantic_result = 0;
    let code_lenses = args.first().is_some_and(|arg| arg == "--code-lenses");
    let inline_values = args.first().is_some_and(|arg| arg == "--inline-values");
    let mut hold_lenses = args.iter().any(|arg| arg == "--hold-resolves");
    let mut held_lenses = Vec::<Value>::new();
    let navigation = args.first().is_some_and(|arg| arg == "--navigation");
    let language_features = args.first().is_some_and(|arg| arg == "--language-features");
    let signature_help = args.first().is_some_and(|arg| arg == "--signature-help");
    let inline_completion = args.first().is_some_and(|arg| arg == "--inline-completion");
    let completion = args.first().is_some_and(|arg| arg == "--completion");
    let formatting_callback = args
        .first()
        .is_some_and(|arg| arg == "--formatting-callback");
    let mut pending_format = None;
    let lifecycle = args.first().is_some_and(|arg| arg == "--lifecycle");
    let mut request_log = if file_operations
        || rename
        || workspace_symbols
        || formatting
        || editing_hooks
        || diagnostics
        || code_actions
        || lifecycle
        || signature_help
        || language_features
        || navigation
        || completion
        || inline_completion
        || action_execution
        || code_lenses
        || inline_values
        || semantic_tokens
        || structural_ranges
        || linked_editing
    {
        Some(std::fs::File::create(&args[2])?)
    } else {
        None
    };
    let mut extra_request_log = args
        .iter()
        .position(|arg| arg == "--request-log")
        .map(|index| std::fs::File::create(&args[index + 1]))
        .transpose()?;
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                return Ok(());
            }
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                length = Some(value.trim().parse::<usize>()?);
            }
        }
        let mut bytes = vec![0; length.ok_or_else(|| anyhow::anyhow!("missing Content-Length"))?];
        input.read_exact(&mut bytes)?;
        let message: Value = serde_json::from_slice(&bytes)?;
        if let Some(log) = &mut extra_request_log {
            writeln!(log, "{message}")?;
            log.flush()?;
        }
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        // Hold initialization until the test has finished attaching the document.
        // Otherwise a fast server can race launch_language_servers and didOpen.
        if method == "initialize" {
            wait_for_gate(&args, "--initialize-gate");
        }
        if file_operations
            || rename
            || workspace_symbols
            || formatting
            || editing_hooks
            || action_execution
            || code_lenses
            || inline_values
            || semantic_tokens
            || structural_ranges
            || linked_editing
            || language_features
            || navigation
        {
            writeln!(request_log.as_mut().unwrap(), "{message}")?;
            request_log.as_mut().unwrap().flush()?;
        }
        if lifecycle {
            writeln!(request_log.as_mut().unwrap(), "{message}")?;
            request_log.as_mut().unwrap().flush()?;
            // Tests can hold initialization until a pre-initialization notification is checked.
            if method == "initialize"
                && let Some(gate) = args.get(3)
            {
                while !std::path::Path::new(gate).exists() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        if method == "initialize" && args.iter().any(|arg| arg == "--reject-initialize") {
            respond(
                &mut output,
                json!({"jsonrpc": "2.0", "id": message["id"],
                "error": {"code": -32603, "message": "test initialization failure"}}),
            )?;
            continue;
        }
        if method == "initialize" && args.iter().any(|arg| arg == "--ignore-initialize") {
            continue;
        }
        if method == "initialize" && args.iter().any(|arg| arg == "--exit-on-initialize") {
            return Ok(());
        }
        match method {
            "textDocument/didOpen" => {
                versions.insert(
                    uri.into(),
                    params["textDocument"]["version"].as_i64().unwrap(),
                );
                documents.insert(
                    uri.into(),
                    params["textDocument"]["text"].as_str().unwrap().into(),
                );
                if code_lenses && args.iter().any(|arg| arg == "--refresh") {
                    respond(
                        &mut output,
                        json!({"jsonrpc": "2.0", "id": "lens-refresh",
                        "method": "workspace/codeLens/refresh", "params": null}),
                    )?;
                }
                if lifecycle {
                    respond(
                        &mut output,
                        json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                            "params": {"uri": uri, "version": params["textDocument"]["version"],
                                "diagnostics": [{"range": range(3, 6), "severity": 2, "message": args[1]}]}
                        }),
                    )?;
                }
                continue;
            }
            "textDocument/didChange" => {
                versions.insert(
                    uri.into(),
                    params["textDocument"]["version"].as_i64().unwrap(),
                );
                documents.insert(
                    uri.into(),
                    params["contentChanges"][0]["text"].as_str().unwrap().into(),
                );
                continue;
            }
            "exit" => return Ok(()),
            _ => {}
        }
        if formatting_callback && message["id"] == "format-config" && !message["method"].is_string()
        {
            let (id, indent) = pending_format.take().unwrap();
            respond(
                &mut output,
                json!({"jsonrpc": "2.0", "id": id,
                "result": [{"range": {"start": {"line": 0, "character": 0},
                    "end": {"line": 1, "character": 0}},
                    "newText": format!("{}formatted\n", " ".repeat(indent))}]}),
            )?;
            continue;
        }
        if formatting_callback && method == "textDocument/formatting" {
            pending_format = Some((
                message["id"].clone(),
                params["options"]["tabSize"].as_u64().unwrap() as usize,
            ));
            // Like yaml-language-server, do not finish until the editor replies.
            respond(
                &mut output,
                json!({"jsonrpc": "2.0", "id": "format-config",
                "method": "workspace/configuration", "params": {"items": [{"section": "[yaml]", "scopeUri": uri}]}}),
            )?;
            continue;
        }
        // Client replies to server requests are logged, but need no response.
        if !message["method"].is_string() {
            continue;
        }
        let Some(id) = message.get("id") else {
            continue;
        };
        if args
            .iter()
            .position(|arg| arg == "--hold-request")
            .is_some_and(|index| args[index + 1] == method)
        {
            continue;
        }
        let text = documents.get(uri).map(|s| s.trim()).unwrap_or_default();
        let end = text.encode_utf16().count();
        if editing_hooks
            && matches!(
                method,
                "textDocument/willSaveWaitUntil"
                    | "textDocument/onTypeFormatting"
                    | "textDocument/formatting"
            )
        {
            if args.iter().any(|arg| arg == "--hooks-ignore") {
                continue;
            }
            wait_for_gate(&args, "--hook-response-gate");
            let response = if args.iter().any(|arg| arg == "--hooks-error") {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": "hook failed"}})
            } else {
                let append = method == "textDocument/willSaveWaitUntil"
                    && args.iter().any(|arg| arg == "--append-hook-name");
                let range = if append {
                    range(0, end)
                } else if args.iter().any(|arg| arg == "--hooks-invalid") {
                    range(6, 3)
                } else if method == "textDocument/willSaveWaitUntil" {
                    range(3, end)
                } else {
                    range(3, 6)
                };
                let replacement = if append {
                    format!("{text} {}", args[1])
                } else if method == "textDocument/willSaveWaitUntil" {
                    "saved".into()
                } else {
                    "formatted".into()
                };
                json!({"jsonrpc": "2.0", "id": id, "result": [{"range": range, "newText": replacement}]})
            };
            respond(&mut output, response)?;
            continue;
        }
        if structural_ranges
            && matches!(
                method,
                "textDocument/foldingRange" | "textDocument/selectionRange"
            )
        {
            wait_for_gate(&args, "--response-gate");
            if args.iter().any(|arg| arg == "--reject-ranges") {
                respond(
                    &mut output,
                    json!({"jsonrpc":"2.0", "id": id,
                    "error":{"code":-32603,"message":"test range failure"}}),
                )?;
                continue;
            }
        }
        if formatting && method == "textDocument/formatting" {
            if args.iter().any(|arg| arg == "--ignore-formatting") {
                continue;
            }
            if args.iter().any(|arg| arg == "--reject-formatting") {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32603, "message": "test formatting failure"}}),
                )?;
                continue;
            }
        }
        if rename && matches!(method, "textDocument/prepareRename" | "textDocument/rename") {
            let gate_arg = if method == "textDocument/rename" {
                "--response-gate"
            } else {
                "--prepare-gate"
            };
            wait_for_gate(&args, gate_arg);
            let result = if method == "textDocument/prepareRename" {
                if args.iter().any(|arg| arg == "--invalid-range") {
                    json!({"start": {"line": 100, "character": 0}, "end": {"line": 100, "character": 3}})
                } else {
                    range(3, 6)
                }
            } else {
                json!({"documentChanges": [{
                    "textDocument": {"uri": uri, "version": versions[uri]},
                    "edits": [{"range": range(3, 6), "newText": params["newName"]}]
                }]})
            };
            respond_result(&mut output, id, result)?;
            continue;
        }
        if workspace_symbols && method == "workspace/symbol" {
            wait_for_gate(&args, "--symbol-query-gate");
            if args.iter().any(|arg| arg == "--hold-symbols") {
                continue;
            }
            let uri = documents
                .keys()
                .next()
                .expect("document opened before symbol query");
            let name = if args.iter().any(|arg| arg == "--symbols-from-text") {
                format!(
                    "{}:{}:{}",
                    args[1],
                    documents[uri].trim(),
                    params["query"].as_str().unwrap()
                )
            } else {
                format!("{}:{}", args[1], params["query"].as_str().unwrap())
            };
            let result = if args.iter().any(|arg| arg == "--unsupported-symbols") {
                json!([
                    {"name": name, "kind": 12, "location": {"uri": uri, "range": range(3, 6)}},
                    {"name": "unsupported", "kind": 12, "location": {"uri": "https://example.com/symbol", "range": range(3, 6)}}
                ])
            } else if args.iter().any(|arg| arg == "--lazy-symbols") {
                json!([
                    {"name": name, "kind": 12, "location": {"uri": uri}, "data": {"provider": args[1], "query": params["query"], "token": [1, {"opaque": true}]}},
                    {"name": format!("{}:other", args[1]), "kind": 12, "location": {"uri": uri}, "data": {"provider": args[1], "query": "other", "token": [1, {"opaque": true}]}}
                ])
            } else {
                json!([{"name": name, "kind": 12, "location": {"uri": uri, "range": range(3, 6)}}])
            };
            if args.iter().any(|arg| arg == "--stream-symbols") {
                respond(
                    &mut output,
                    json!({"jsonrpc":"2.0", "method":"$/progress", "params":{
                        "token":params["partialResultToken"], "value":result
                    }}),
                )?;
                wait_for_gate(&args, "--symbol-final-gate");
                if args.iter().any(|arg| arg == "--symbol-second-batch") {
                    let mut second = result.clone();
                    for item in second.as_array_mut().unwrap() {
                        item["name"] = json!(format!("{}:second", item["name"].as_str().unwrap()));
                    }
                    respond(
                        &mut output,
                        json!({"jsonrpc":"2.0", "method":"$/progress", "params":{
                            "token":params["partialResultToken"], "value":second
                        }}),
                    )?;
                }
                if args.iter().any(|arg| arg == "--symbol-stream-error") {
                    respond(
                        &mut output,
                        json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32603, "message":"symbol query failed"}}),
                    )?;
                } else {
                    respond_result(&mut output, id, Value::Null)?;
                }
            } else {
                respond_result(&mut output, id, result)?;
            }
            continue;
        }
        if method == "workspace/diagnostic" && diagnostics {
            writeln!(request_log.as_mut().unwrap(), "{message}")?;
            request_log.as_mut().unwrap().flush()?;
            let index = args
                .iter()
                .position(|arg| arg == "--workspace-diagnostics")
                .unwrap();
            let reports = diagnostic_reports(&args[index + 1])?;
            for batch in reports["batches"].as_array().into_iter().flatten() {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "method": "$/progress",
                    "params": {"token": params["partialResultToken"], "value": batch}}),
                )?;
            }
            if reports["hold"].as_bool() == Some(true) {
                continue;
            }
            let response = if let Some(error) = reports.get("error") {
                json!({"jsonrpc": "2.0", "id": id, "error": error})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "result": reports["result"]})
            };
            respond(&mut output, response)?;
            continue;
        }
        if workspace_symbols && method == "workspaceSymbol/resolve" {
            if args.iter().any(|arg| arg == "--hold-symbol-resolves") {
                continue;
            }
            let mut symbol = params.clone();
            symbol["location"]["range"] = range(3, 6);
            respond_result(&mut output, id, symbol)?;
            continue;
        }
        if method == "textDocument/diagnostic" && diagnostics {
            let label = &args[1];
            let version = versions.get(uri).copied().unwrap_or_default();
            writeln!(request_log.as_mut().unwrap(), "{params}")?;
            request_log.as_mut().unwrap().flush()?;
            let count = diagnostic_requests
                .entry((uri.into(), version))
                .or_default();
            *count += 1;
            if text.contains("retry") && *count == 1 {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id, "error": {
                        "code": lsp_client::lsp::error_codes::SERVER_CANCELLED,
                        "message": "retry once", "data": {"retriggerRequest": true}
                    }}),
                )?;
                continue;
            }
            let result_id = format!("{label}:{version}");
            let mut result = if params["previousResultId"].as_str() == Some(&result_id) {
                json!({"kind": "unchanged", "resultId": result_id})
            } else {
                let mut report = json!({"kind": "full", "items": [{
                    "range": range(3, end), "severity": 2, "message": format!("{label}: {text}")
                }]});
                if !text.contains("no-id") {
                    report["resultId"] = json!(result_id);
                }
                report
            };
            if let Some(index) = args.iter().position(|arg| arg == "--related-diagnostics") {
                result["relatedDocuments"] = diagnostic_reports(&args[index + 1])?;
            }
            respond_result(&mut output, id, result)?;
            continue;
        }
        if method == "textDocument/codeAction"
            && action_execution
            && args.iter().any(|arg| arg == "--presentation-details")
        {
            respond(
                &mut output,
                json!({"jsonrpc": "2.0", "id": id, "result": [
                    {"title": "Generated fix", "kind": "quickfix", "tags": [1, 999],
                     "edit": {"changes": {}}, "command": {"title": "Apply fix", "command": "fixture.command", "tooltip": "Run generated fix"}},
                    {"title": "Unknown annotation", "kind": "quickfix", "tags": [999], "edit": {"changes": {}}},
                    {"title": "Run helper", "command": "fixture.command", "tooltip": "Explain helper"},
                    {"title": "disabled", "kind": "source.first", "disabled": {"reason": "unavailable"}}
                ]}),
            )?;
            continue;
        }
        if method == "textDocument/codeAction"
            && action_execution
            && args.iter().any(|a| a == "--multicursor-actions")
        {
            let start = params["range"]["start"]["character"].as_u64().unwrap();
            let mut result = Vec::new();
            if start != 1 {
                for (title, from, to, replacement) in [
                    ("expand", start, start + 1, "XX"),
                    ("shared", 0, 0, "import\n"),
                    ("overlap", 0, 3, if start == 0 { "first" } else { "second" }),
                    ("late command", start, start + 1, "XX"),
                    ("resolve failure", start, start + 1, "XX"),
                    ("different kind", start, start + 1, "XX"),
                    ("invalid target", start, start + 1, "XX"),
                    ("resource operation", start, start + 1, "XX"),
                    (
                        "conflicting insert",
                        0,
                        0,
                        if start == 0 { "first" } else { "second" },
                    ),
                    ("ambiguous", start, start + 1, "XX"),
                ] {
                    let mut action = json!({
                        "title": title, "kind": "refactor.rewrite",
                        "data": {"uri": uri, "version": versions[uri], "multicursor": true,
                            "start": from, "end": to, "replacement": replacement}
                    });
                    if title == "resolve failure" && start == 2 {
                        action["data"]["failure"] = json!("resolve");
                    }
                    if title == "late command" {
                        action["data"]["lateCommand"] = json!(true);
                    }
                    if title == "different kind" && start == 2 {
                        action["kind"] = json!("quickfix");
                    }
                    if args.iter().any(|a| a == "--no-resolve") {
                        action["edit"] = json!({"changes": {uri: [{
                            "range": range(from as usize, to as usize), "newText": replacement
                        }]}});
                    }
                    if title == "ambiguous" && start == 0 {
                        result.push(action.clone());
                    }
                    result.push(action);
                }
                result.push(json!({"title": "command", "command": "fixture.command"}));
                result.push(json!({"title": "disabled", "disabled": {"reason": "unavailable"}}));
                if start == 2 {
                    result.push(json!({"title": "secondary only", "kind": "quickfix",
                        "data": {"uri": uri, "version": versions[uri], "multicursor": true,
                            "start": start, "end": start + 1, "replacement": "YY"}}));
                }
            }
            respond_result(&mut output, id, Value::Array(result))?;
            continue;
        }
        if method == "textDocument/codeAction"
            && action_execution
            && args.iter().any(|arg| arg == "--save-actions")
        {
            let kind = params["context"]["only"][0].as_str().unwrap_or("quickfix");
            let replacement = if kind == "source.first" {
                "first"
            } else {
                "second"
            };
            let result = json!([
                {"title": "disabled", "kind": kind, "disabled": {"reason": "unavailable"}},
                {"title": "unclassified"},
                {"title": "unrelated", "kind": format!("{kind}Other")},
                {"title": "command", "command": "fixture.command"},
                {"title": "save action", "kind": format!("{kind}.fixture"),
                    "data": {"uri": uri, "version": versions[uri],
                        "replacement": replacement, "end": end}}
            ]);
            respond_result(&mut output, id, result)?;
            continue;
        }
        if method == "textDocument/codeAction" && code_actions {
            writeln!(request_log.as_mut().unwrap(), "{params}")?;
            request_log.as_mut().unwrap().flush()?;
            let result = if text.contains("disabled") {
                json!([{"title": "disabled", "disabled": {"reason": "unavailable"}}])
            } else if text.contains("empty") {
                Value::Null
            } else if text.contains("command") {
                json!([{"title": "command", "command": "fixture.command"}])
            } else {
                json!([{"title": format!("{}: {text}", args[1]), "kind": "quickfix"}])
            };
            respond_result(&mut output, id, result)?;
            continue;
        }
        if method == "textDocument/inlineCompletion" && inline_completion {
            writeln!(request_log.as_mut().unwrap(), "{params}")?;
            request_log.as_mut().unwrap().flush()?;
            let mut items = json!([
                {"insertText": {"kind": "snippet", "value": "${1:print}"}},
                {"range": range(3, 5), "insertText": "print(界)\n\treturn 1"},
                {"insertText": "# alternate"}
            ]);
            if let Some(selected) = params["context"].get("selectedCompletionInfo") {
                items = json!([
                    {"range": range(0, 0), "insertText": "wrong range"},
                    {"range": selected["range"], "insertText": "wrong text"},
                    {"range": selected["range"], "insertText": format!("{}(界)", selected["text"].as_str().unwrap())}
                ]);
            }
            let result = if text.contains("empty") {
                Value::Null
            } else if args.iter().any(|arg| arg == "--array") {
                items
            } else {
                json!({"items": items})
            };
            respond_result(&mut output, id, result)?;
            continue;
        }
        if navigation
            && matches!(
                method,
                "textDocument/definition"
                    | "textDocument/declaration"
                    | "textDocument/typeDefinition"
                    | "textDocument/implementation"
                    | "textDocument/references"
                    | "textDocument/documentHighlight"
            )
        {
            if args.iter().any(|arg| arg == "--hold-navigation") {
                continue;
            }
            if args.iter().any(|arg| arg == "--navigation-error") {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": "navigation fixture error"}}),
                )?;
                continue;
            }
            let start = if args.iter().any(|arg| arg == "--utf-8") {
                5
            } else {
                3
            };
            let location = json!({"uri": uri, "range": range(start, start + 3)});
            let result = if text.contains("empty") {
                Value::Null
            } else {
                match method {
                    "textDocument/definition" => location,
                    "textDocument/declaration" | "textDocument/references" => json!([location]),
                    "textDocument/implementation" => {
                        json!([{"uri": "https://example.com/unsupported", "range": range(0, 1)}, location])
                    }
                    "textDocument/typeDefinition" => {
                        json!([{"targetUri": uri, "targetRange": range(start, start + 3), "targetSelectionRange": range(start + 1, start + 2)}])
                    }
                    "textDocument/documentHighlight" if text.contains("invalid") => {
                        json!([{"range": {"start": {"line": 999, "character": 0}, "end": {"line": 999, "character": 1}}}])
                    }
                    "textDocument/documentHighlight" => {
                        json!([{"range": range(start, start + 3)}, {"range": range(start + 4, start + 7)}])
                    }
                    _ => unreachable!(),
                }
            };
            respond_result(&mut output, id, result)?;
            continue;
        }
        if language_features && matches!(method, "textDocument/hover" | "textDocument/inlayHint") {
            if method == "textDocument/hover" && args.iter().any(|arg| arg == "--hold-hover") {
                continue;
            }
            let result = if text.contains("empty") {
                Value::Null
            } else if method == "textDocument/hover" {
                json!({"contents": {"kind": "plaintext", "value": format!("{}: {text}", args[1])}})
            } else {
                json!([{"position": {"line": 0, "character": 3}, "label": format!("界é {text}"), "kind": 1, "paddingLeft": true, "paddingRight": true}])
            };
            respond_result(&mut output, id, result)?;
            if method == "textDocument/inlayHint"
                && !inlay_refresh_sent
                && args.iter().any(|arg| arg == "--inlay-refresh")
            {
                inlay_refresh_sent = true;
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": "inlay-refresh",
                        "method": "workspace/inlayHint/refresh", "params": null}),
                )?;
            }
            continue;
        }
        if method == "textDocument/signatureHelp" && signature_help {
            writeln!(request_log.as_mut().unwrap(), "{params}")?;
            request_log.as_mut().unwrap().flush()?;
            wait_for_gate(&args, "--response-gate");
            if text.contains("error") {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32603, "message": "signature fixture error"}}),
                )?;
                continue;
            }
            let result = if text.contains("empty") {
                Value::Null
            } else if text.contains("no-signatures") {
                json!({"signatures": []})
            } else {
                json!({"signatures": [
                    {"label": format!("{}: {text}", args[1]), "documentation": "first signature"},
                    {"label": "second(value)", "parameters": [{"label": "value"}], "activeParameter": 0}
                ], "activeSignature": 0})
            };
            respond_result(&mut output, id, result)?;
            continue;
        }
        if method == "textDocument/completion" && (completion || inline_completion) {
            writeln!(request_log.as_mut().unwrap(), "{params}")?;
            request_log.as_mut().unwrap().flush()?;
            wait_for_gate(&args, "--response-gate");
            if let Some(index) = args.iter().position(|arg| arg == "--completion-result") {
                let result: Value = serde_json::from_slice(&std::fs::read(&args[index + 1])?)?;
                respond_result(&mut output, id, result)?;
                continue;
            }
            let end = params["position"]["character"].as_u64().unwrap() as usize;
            let line = params["position"]["line"].as_u64().unwrap();
            let mut edit_range = range(end.saturating_sub(2), end);
            edit_range["start"]["line"] = json!(line);
            edit_range["end"]["line"] = json!(line);
            let (first, second) = if inline_completion {
                ("print", "println")
            } else {
                ("apple", "apricot")
            };
            if text.contains("postfix") {
                let source_line = text.lines().nth(line as usize).unwrap();
                let dot = source_line[..source_line.rfind('.').unwrap()]
                    .encode_utf16()
                    .count();
                let mut prefix_range = edit_range.clone();
                prefix_range["start"]["character"] = json!(dot - 1);
                prefix_range["end"]["character"] = json!(dot + 1);
                edit_range["start"]["character"] = json!(dot + 1);
                // gopls replaces the suffix and deletes the receiver separately.
                let items: Vec<_> = [
                    ("for!", "for ${1:i} := range x {\n\t$0\n}"),
                    ("forr!", "for ${1:i}, ${2:v} := range x {\n\t$0\n}"),
                ]
                .into_iter()
                .map(|(label, snippet)| {
                    json!({
                        "label": label, "insertTextFormat": 2,
                        "textEdit": {"range": edit_range, "newText": snippet},
                        "additionalTextEdits": [{"range": prefix_range, "newText": ""}]
                    })
                })
                .collect();
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id, "result": {"isIncomplete": false, "items": items}}),
                )?;
                continue;
            }
            let snippet = text.contains("snippet");
            let first_text = if snippet {
                format!("{first}(${{1:value}})$0")
            } else {
                first.to_owned()
            };
            let second_text = if snippet {
                format!("{second}(${{1:value}})$0")
            } else {
                second.to_owned()
            };
            let result = json!({"isIncomplete": text.contains("incomplete"), "items": [
                {"label": first, "sortText": "2", "detail": args[1],
                    "textEdit": {"range": edit_range, "newText": first_text}, "insertTextFormat": if snippet { 2 } else { 1 }},
                {"label": second, "sortText": "1", "detail": args[1],
                    "textEdit": {"range": edit_range, "newText": second_text}, "insertTextFormat": if snippet { 2 } else { 1 }}
            ]});
            respond_result(&mut output, id, result)?;
            continue;
        }
        if method == "completionItem/resolve" && (completion || inline_completion) {
            if let Some(index) = args.iter().position(|arg| arg == "--resolve-log") {
                let mut log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&args[index + 1])?;
                writeln!(log, "{params}")?;
            }
            wait_for_gate(&args, "--resolve-gate");
            if args.iter().any(|arg| arg == "--resolve-error") {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32603, "message": "completion resolve failed"}}),
                )?;
                continue;
            }
            let mut item = params.clone();
            item["documentation"] = json!("resolved documentation");
            if documents.values().any(|text| text.starts_with("imports"))
                || args.iter().any(|arg| arg == "--resolve-additional-edits")
            {
                item["additionalTextEdits"] =
                    json!([{"range":range(0, 0), "newText":"use fruit;\n"}]);
            }
            respond_result(&mut output, id, item)?;
            continue;
        }
        if method == "codeAction/resolve" && action_execution {
            wait_for_gate(&args, "--response-gate");
            if params["data"]["failure"] == "resolve" {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32603, "message": "fixture resolve failure"}}),
                )?;
            } else {
                let mut action = params.clone();
                let version = if params["data"]["failure"] == "edit" {
                    -1
                } else {
                    params["data"]["version"].as_i64().unwrap()
                };
                action["edit"] = json!({"documentChanges": [{
                    "textDocument": {"uri": params["data"]["uri"], "version": version},
                    "edits": [{"range": range(params["data"]["start"].as_u64().unwrap_or(0) as usize, params["data"]["end"].as_u64().unwrap_or(3) as usize),
                        "newText": params["data"]["replacement"].as_str().unwrap_or("fixed")}]
                }]});
                if params["title"] == "invalid target" {
                    let other_uri = format!("{}-other", params["data"]["uri"].as_str().unwrap());
                    action["edit"]["documentChanges"].as_array_mut().unwrap().push(json!({
                        "textDocument": {"uri": other_uri, "version": null},
                        "edits": [{"range": {"start": {"line": 99, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "invalid"}]
                    }));
                }
                if params["title"] == "resource operation" {
                    action["edit"]["documentChanges"].as_array_mut().unwrap().push(json!({
                        "kind": "create", "uri": format!("{}-created", params["data"]["uri"].as_str().unwrap())
                    }));
                }
                if params["data"]["multicursor"] != true || params["data"]["lateCommand"] == true {
                    action["command"] =
                        json!({"title": "after edit", "command": "fixture.command"});
                }
                respond_result(&mut output, id, action)?;
            }
            continue;
        }
        if method == "codeLens/resolve" && code_lenses {
            if hold_lenses {
                held_lenses.push(message.clone());
                continue;
            }
            let mut lens = params.clone();
            // Fail one lens and leave another unresolved to exercise partial results.
            if params["data"]["kind"] == "failure" {
                respond(
                    &mut output,
                    json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32603, "message": "fixture resolution failure"}}),
                )?;
                continue;
            }
            if params["data"]["kind"] != "empty" {
                lens["command"] = json!({"title": format!("{} resolved", args[1]),
                    "command": "fixture.lens", "tooltip": "Run resolved lens", "arguments": [args[1], params["data"]]});
            }
            respond_result(&mut output, id, lens)?;
            continue;
        }
        if method == "workspace/executeCommand"
            && code_lenses
            && params["command"] == "fixture.release-lenses"
        {
            hold_lenses = false;
            for message in std::mem::take(&mut held_lenses) {
                let mut lens = message["params"].clone();
                lens["command"] = json!({"title": format!("{} resolved", args[1]),
                    "command": "fixture.lens", "arguments": [args[1], lens["data"]]});
                respond_result(&mut output, &message["id"], lens)?;
            }
        }
        if method == "documentLink/resolve"
            && let Some(index) = args.iter().position(|arg| arg == "--resolve-link-gate")
        {
            let gate = std::path::Path::new(&args[index + 1]);
            std::fs::write(gate.with_extension("started"), "ready")?;
            while !gate.exists() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        // Test documents start with an emoji and a space: UTF-16 column 3 is char 2.
        if file_operations && method.starts_with("workspace/will") {
            wait_for_gate(&args, "--preflight-gate");
            let result = if let Some(index) = args.iter().position(|arg| arg == "--preflight-edit")
            {
                serde_json::from_slice(&std::fs::read(&args[index + 1])?)?
            } else if method == "workspace/willRenameFiles" {
                let uri = params["files"][0]["oldUri"].as_str().unwrap();
                if documents
                    .get(uri)
                    .is_some_and(|text| text.starts_with("reject"))
                {
                    json!({"documentChanges":[{"textDocument":{"uri":uri,"version":-1}, "edits":[{"range":range(0, 0),"newText":"prepared "}]}]})
                } else {
                    json!({"changes": {uri: [{"range": range(0, 0), "newText": "prepared "}]}})
                }
            } else {
                Value::Null
            };
            respond_result(&mut output, id, result)?;
            continue;
        }
        if method == "textDocument/documentSymbol" && document_symbols {
            if args.iter().any(|arg| arg == "--hold-symbols") {
                continue;
            }
            if args.iter().any(|arg| arg == "--fail-symbols") {
                respond(
                    &mut output,
                    json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32603,"message":"provider failed"}}),
                )?;
                continue;
            }
            let name = if args.iter().any(|arg| arg == "--symbols-from-text") {
                format!("symbol:{text}")
            } else {
                args[1].clone()
            };
            let result = if args.iter().any(|arg| arg == "--flat-symbols") {
                json!([{"name":args[1], "kind":12, "location":{"uri":format!("{uri}.other"), "range":range(0, 1)}}])
            } else {
                json!([{"name":name, "kind":12, "range":range(0, end), "selectionRange":range(0, 1), "children":[{"name":"child", "kind":12,"range":range(1, 2), "selectionRange":range(1, 2)}]}])
            };
            respond_result(&mut output, id, result)?;
            continue;
        }
        let mut result = match method {
            "initialize" if editing_hooks => json!({"capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {
                    "openClose": true, "change": 1, "save": {"includeText": true},
                    "willSave": !args.iter().any(|arg| arg == "--no-save-hooks"),
                    "willSaveWaitUntil": !args.iter().any(|arg| arg == "--no-save-hooks" || arg == "--will-save-only")
                },
                "documentOnTypeFormattingProvider": {"firstTriggerCharacter": ";", "moreTriggerCharacter": ["\n"]},
                "documentFormattingProvider": args.iter().any(|arg| arg == "--document-formatting")
            }}),
            "initialize" if file_operations => json!({"capabilities": {
                "textDocumentSync": 1, "workspace": {"fileOperations": {
                    "willRename": {"filters": [{"pattern": {"glob": "**"}}]},
                    "didRename": {"filters": [{"pattern": {"glob": "**"}}]},
                    "willCreate": {"filters": [{"pattern": {"glob": "**"}}]},
                    "didCreate": {"filters": [{"pattern": {"glob": "**"}}]},
                    "willDelete": {"filters": [{"pattern": {"glob": "**"}}]},
                    "didDelete": {"filters": [{"pattern": {"glob": "**"}}]}
                }}
            }}),
            "initialize" if rename => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "renameProvider": {"prepareProvider": !args.iter().any(|arg| arg == "--no-prepare")}
            }}),
            "initialize" if workspace_symbols => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1, "workspaceSymbolProvider": {"resolveProvider": args.iter().any(|arg| arg == "--lazy-symbols")}
            }}),
            "initialize" if document_symbols => {
                json!({"capabilities":{"textDocumentSync":1,"documentSymbolProvider":true}})
            }
            "initialize" if formatting => json!({"capabilities": {
                "textDocumentSync": 1, "documentRangeFormattingProvider":
                    if args.iter().any(|arg| arg == "--multi-range-formatting") {
                        json!({"rangesSupport":true})
                    } else { json!(true) },
                "documentFormattingProvider": args.iter().any(|arg| arg == "--document-formatting")
            }}),
            "textDocument/formatting" if formatting => json!([{
                "range": range(3, 6), "newText": "formatted"
            }]),
            "textDocument/rangeFormatting" if formatting => json!([{
                "range": params["range"], "newText": "formatted"
            }]),
            "textDocument/rangesFormatting" if formatting => {
                let mut edits: Vec<_> = params["ranges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|range| json!({"range":range,"newText":"formatted"}))
                    .collect();
                if args.iter().any(|arg| arg == "--overlapping-range-edits") {
                    edits.push(edits[0].clone());
                }
                json!(edits)
            }
            "initialize" if inline_values => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1, "inlineValueProvider": {}
            }}),
            "textDocument/inlineValue" if inline_values => json!([
                {"range": range(3, 9), "text": "runtime"},
                {"range": range(3, 9), "caseSensitiveLookup": true},
                {"range": range(3, 9), "variableName": "ANSWER", "caseSensitiveLookup": false},
                {"range": range(3, 9), "expression": "answer + 1"}
            ]),
            "initialize" if linked_editing => json!({"capabilities": {
                "positionEncoding":"utf-16", "textDocumentSync":1,
                "linkedEditingRangeProvider": !args.iter().any(|arg| arg == "--unsupported"),
                "completionProvider": if args.iter().any(|arg| arg == "--linked-completion") {json!({})} else {Value::Null}
            }}),
            "textDocument/completion" if linked_editing => {
                let linked_text = &documents[uri];
                let opening = linked_text.find('<').unwrap() + 1;
                let closing = linked_text.rfind("</").unwrap() + 2;
                let from = if params["position"]["character"].as_u64().unwrap() as usize
                    >= linked_text[..closing].encode_utf16().count()
                {
                    closing
                } else {
                    opening
                };
                let end = from + linked_text[from..].find('>').unwrap();
                let mut item = json!({"label":"tagged", "textEdit": {
                    "range": range(linked_text[..from].encode_utf16().count(), linked_text[..end].encode_utf16().count()),
                    "newText":"tagged"}});
                if args.iter().any(|arg| arg == "--linked-completion-final") {
                    item["insertTextFormat"] = json!(2);
                    item["textEdit"]["newText"] = json!("tagged$0");
                }
                if args.iter().any(|arg| arg == "--linked-completion-as-is") {
                    item["insertTextMode"] = json!(1);
                }
                if args.iter().any(|arg| arg == "--linked-completion-import") {
                    item["additionalTextEdits"] =
                        json!([{"range":range(0,0),"newText":"imported\n"}]);
                }
                json!({"isIncomplete":false,"items":[item]})
            }
            "textDocument/linkedEditingRange" if linked_editing => {
                wait_for_gate(&args, "--linked-response-gate");
                if args.iter().any(|arg| arg == "--null") {
                    Value::Null
                } else {
                    let mut ranges = vec![range(4, 7), range(14, 17)];
                    if args.iter().any(|arg| arg == "--reverse") {
                        ranges[0] = range(7, 4);
                    }
                    if args.iter().any(|arg| arg == "--overlap") {
                        ranges[1] = range(5, 7);
                    }
                    if args.iter().any(|arg| arg == "--mismatch") {
                        ranges[1] = range(14, 16);
                    }
                    if args.iter().any(|arg| arg == "--bad-position") {
                        ranges[0] = range(1, 7);
                    }
                    let pattern = if args.iter().any(|arg| arg == "--bad-pattern") {
                        "(?=tag)tag"
                    } else {
                        "[a-zA-Z0-9:_-]+"
                    };
                    if args.iter().any(|arg| arg == "--no-pattern") {
                        json!({"ranges":ranges})
                    } else {
                        json!({"ranges":ranges,"wordPattern":pattern})
                    }
                }
            }
            "initialize" if structural_ranges => json!({"capabilities": {
                "positionEncoding":"utf-16", "textDocumentSync":1,
                "foldingRangeProvider":true, "selectionRangeProvider":true
            }}),
            "textDocument/foldingRange" if structural_ranges => {
                folding_requests += 1;
                if args.iter().any(|arg| arg == "--null-ranges") {
                    Value::Null
                } else if args.iter().any(|arg| arg == "--empty-ranges") {
                    json!([])
                } else if args.iter().any(|arg| arg == "--character-folds") {
                    json!([
                        {"startLine":0,"startCharacter":3,"endLine":0,"endCharacter":9,"kind":"comment","collapsedText":if folding_requests > 1 { "updated" } else { "界e\u{301}" }},
                        {"startLine":0,"startCharacter":10,"endLine":0,"endCharacter":15,"kind":"imports","collapsedText":"use"},
                        {"startLine":1,"startCharacter":1,"endLine":2,"endCharacter":1,"kind":"region","collapsedText":"region"},
                        {"startLine":3,"endLine":4,"kind":"server-custom"}
                    ])
                } else if args.iter().any(|arg| arg == "--nested-folds") {
                    json!([{"startLine":0,"endLine":3},{"startLine":1,"endLine":2}])
                } else {
                    let start = if args.iter().any(|arg| arg == "--refresh-folds")
                        && folding_requests > 1
                    {
                        1
                    } else {
                        0
                    };
                    json!([
                        {"startLine":start,"endLine":2},
                        {"startLine":5,"endLine":1}, {"startLine":0,"endLine":999}
                    ])
                }
            }
            "textDocument/selectionRange" if structural_ranges => {
                if args.iter().any(|arg| arg == "--null-ranges") {
                    Value::Null
                } else if args.iter().any(|arg| arg == "--invalid-ranges") {
                    json!([{ "range": range(8, 3) }])
                } else {
                    Value::Array(message["params"]["positions"].as_array().unwrap().iter().map(|position| {
                        let line = position["line"].as_u64().unwrap();
                        let (start, end) = if position["character"].as_u64().unwrap() >= 9 { (9,13) } else { (3,8) };
                        let at_line = |start, end| json!({"start":{"line":line,"character":start},"end":{"line":line,"character":end}});
                        json!({"range":at_line(start,end),"parent":{"range":at_line(0,13)}})
                    }).collect())
                }
            }
            "initialize" if semantic_tokens => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "semanticTokensProvider": {
                    "legend": {"tokenTypes": ["method"], "tokenModifiers": ["definition", "deprecated"]},
                    "full": if args.iter().any(|arg| arg == "--range-only") { json!(false) }
                        else if semantic_deltas { json!({"delta": !args.iter().any(|arg| arg == "--no-delta-support")}) }
                        else { json!(true) },
                    "range": if args.iter().any(|arg| arg == "--range-options") { json!({}) } else { json!(true) }
                }
            }}),
            "textDocument/semanticTokens/full" | "textDocument/semanticTokens/range"
                if semantic_tokens =>
            {
                semantic_result += 1;
                let mut result = if text.contains("no tokens") {
                    json!({"data": []})
                } else {
                    json!({"data": [0, 3, if text.contains("😀 x") { 1 } else { 3 }, 0, 3]})
                };
                if semantic_deltas && !args.iter().any(|arg| arg == "--no-result-id") {
                    result["resultId"] = json!(format!("full-{semantic_result}"));
                }
                result
            }
            "textDocument/semanticTokens/full/delta" if semantic_deltas => {
                semantic_result += 1;
                if args.iter().any(|arg| arg == "--null-delta") {
                    Value::Null
                } else if args.iter().any(|arg| arg == "--delta-full") {
                    json!({"resultId": format!("delta-{semantic_result}"), "data": [0, 3, 1, 0, 3]})
                } else if args.iter().any(|arg| arg == "--invalid-delta") {
                    json!({"resultId": "invalid", "edits": [{"start": 999, "deleteCount": 1}]})
                } else if args.iter().any(|arg| arg == "--malformed-delta") {
                    json!({"edits": [{"start": "invalid", "deleteCount": 1}]})
                } else {
                    let edits = if text.contains("no tokens") {
                        json!([{"start": 0, "deleteCount": 5}])
                    } else {
                        json!([{"start": 2, "deleteCount": 1, "data": [if text.contains("😀 x") { 1 } else { 3 }]}])
                    };
                    json!({"resultId": format!("delta-{semantic_result}"), "edits": edits})
                }
            }
            "initialize" if code_lenses => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "codeLensProvider": {"resolveProvider": !args.iter().any(|arg| arg == "--no-resolve")},
                "executeCommandProvider": {"commands": ["fixture.lens"]}
            }}),
            "initialize" if action_execution => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "codeActionProvider": {
                    "resolveProvider": !args.iter().any(|arg| arg == "--no-resolve"),
                    "documentation": if args.iter().any(|arg| arg == "--presentation-details") { json!([
                        {"kind": "quickfix", "command": {"title": "Quick fix help", "command": "fixture.documentation", "tooltip": "Learn about quick fixes"}},
                        {"kind": "source", "command": {"title": "Source help", "command": "fixture.documentation"}},
                        {"kind": "source.first", "command": {"title": "First source help", "command": "fixture.documentation"}}
                    ]) } else { Value::Null }
                },
                "executeCommandProvider": {"commands": ["fixture.command"]}
            }}),
            "initialize" if inline_completion => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "inlineCompletionProvider": true,
                "completionProvider": {"resolveProvider": !args.iter().any(|arg| arg == "--no-resolve")}
            }}),
            "initialize" if formatting_callback => json!({"capabilities": {
                "textDocumentSync": 1, "documentFormattingProvider": true
            }}),
            "initialize" if completion => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "completionProvider": {"triggerCharacters": ["."], "resolveProvider": true}
            }}),
            "initialize" if navigation => json!({"capabilities": {
                "textDocumentSync": 1,
                "positionEncoding": if args.iter().any(|arg| arg == "--utf-8") { "utf-8" } else { "utf-16" },
                "definitionProvider": true, "declarationProvider": true,
                "typeDefinitionProvider": if args.iter().any(|arg| arg == "--registration-options") {
                    json!({"documentSelector": null, "id": "types"})
                } else { json!(true) },
                "implementationProvider": if args.iter().any(|arg| arg == "--registration-options") {
                    json!({"documentSelector": null, "id": "implementations"})
                } else { json!(true) },
                "referencesProvider": true, "documentHighlightProvider": true
            }}),
            "initialize" if language_features => json!({"capabilities": {
                "textDocumentSync": 1, "positionEncoding": "utf-16", "hoverProvider": true, "inlayHintProvider": true
            }}),
            "initialize" if signature_help => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "signatureHelpProvider": {"triggerCharacters": ["(", ","], "retriggerCharacters": [")"]}
            }}),
            "initialize" if lifecycle => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1, "documentSymbolProvider": true
            }}),
            "initialize" if code_actions => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1, "codeActionProvider": true
            }}),
            "initialize" if diagnostics => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "diagnosticProvider": {"identifier": args[1], "workspaceDiagnostics": args.iter().any(|arg| arg == "--workspace-diagnostics"),
                    "interFileDependencies": args.iter().any(|arg| arg == "--inter-file")}
            }}),
            "initialize" => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "documentSymbolProvider": true, "documentHighlightProvider": true,
                "colorProvider": true, "documentLinkProvider": {"resolveProvider": args.iter().any(|arg| arg == "--resolve-links")}, "codeLensProvider": {}
            }}),
            "textDocument/documentSymbol" => json!([{
                "name": text, "kind": 12, "range": range(0, end), "selectionRange": range(3, end)
            }]),
            "textDocument/documentHighlight" => json!([
                {"range": range(3, 5)}, {"range": range(4, end)}
            ]),
            "textDocument/documentColor" => json!([{
                "range": range(3, end), "color": {"red": 1.0, "green": 0.0, "blue": 0.0, "alpha": 1.0}
            }]),
            "textDocument/documentLink" if args.iter().any(|arg| arg == "--resolve-links") => {
                json!([{
                    "range": range(3, end), "data": {"target": format!("{uri}.resolved")}
                }])
            }
            "documentLink/resolve" => {
                let mut link = params.clone();
                link["target"] = params["data"]["target"].clone();
                link["tooltip"] = json!("Open *resolved* [target]");
                link
            }
            "textDocument/documentLink" => json!([{
                "range": range(3, end), "target": "https://example.test/target",
                "tooltip": "Open *example* [target]"
            }]),
            "textDocument/codeLens" if code_lenses && text.contains("no lenses") => Value::Null,
            "textDocument/codeLens"
                if code_lenses && args.iter().any(|arg| arg == "--per-line") =>
            {
                Value::Array(
                    documents
                        .get(uri)
                        .unwrap()
                        .lines()
                        .enumerate()
                        .map(|(line, text)| {
                            json!({"range": {"start": {"line": line, "character": 0},
                        "end": {"line": line, "character": text.encode_utf16().count()}},
                        "data": {"server": args[1], "kind": "resolve", "line": line}})
                        })
                        .collect(),
                )
            }
            "textDocument/codeLens"
                if code_lenses && args.iter().any(|arg| arg == "--multi-line") =>
            {
                json!([
                    {"range": {"start": {"line": 0, "character": 3},
                               "end": {"line": 1, "character": 0}},
                     "command": {"title": "next line", "command": "fixture.lens"}},
                    {"range": {"start": {"line": 0, "character": 3},
                               "end": {"line": 99, "character": 0}},
                     "command": {"title": "past end", "command": "fixture.lens"}}
                ])
            }
            "textDocument/codeLens" if code_lenses => json!([
                {"range": range(3, end), "command": {"title": format!("{} ready", args[1]),
                    "command": "fixture.lens", "tooltip": "Run ready lens", "arguments": [args[1]]}},
                {"range": range(3, end), "data": {"server": args[1], "kind": "resolve"}},
                {"range": range(3, end), "data": {"server": args[1], "kind": "empty"}},
                {"range": range(3, end), "data": {"server": args[1], "kind": "failure"}},
                {"range": {"start": {"line": 99, "character": 0}, "end": {"line": 99, "character": 0}}}
            ]),
            "textDocument/codeLens" => json!([{"range": range(3, end)}]),
            _ => Value::Null,
        };
        if method == "initialize" && args.iter().any(|arg| arg == "--workspace-folders") {
            result["capabilities"]["workspace"] = json!({"workspaceFolders": {
                "supported": true,
                "changeNotifications": !args.iter().any(|arg| arg == "--no-workspace-notifications")
            }});
        }
        respond_result(&mut output, id, result)?;
    }
}
