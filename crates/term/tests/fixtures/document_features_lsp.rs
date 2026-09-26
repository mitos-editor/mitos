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

fn range(start: usize, end: usize) -> Value {
    json!({"start": {"line": 0, "character": start}, "end": {"line": 0, "character": end}})
}

fn main() -> anyhow::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut documents = HashMap::<String, String>::new();
    let mut versions = HashMap::<String, i64>::new();
    let mut diagnostic_requests = HashMap::<(String, i64), usize>::new();
    // Optional pull-diagnostic mode leaves the document-feature fixture unchanged.
    let args: Vec<_> = std::env::args().skip(1).collect();
    let diagnostics = args.first().is_some_and(|arg| arg == "--diagnostics");
    let mut request_log = if diagnostics {
        Some(std::fs::File::create(&args[2])?)
    } else {
        None
    };
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
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        // Hold initialization until the test has finished attaching the document.
        // Otherwise a fast server can race launch_language_servers and didOpen.
        if method == "initialize"
            && let Some(index) = args.iter().position(|arg| arg == "--initialize-gate")
        {
            while !std::path::Path::new(&args[index + 1]).exists() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
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
        let Some(id) = message.get("id") else {
            continue;
        };
        let text = documents.get(uri).map(|s| s.trim()).unwrap_or_default();
        let end = text.encode_utf16().count();
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
            let result = if params["previousResultId"].as_str() == Some(&result_id) {
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
            respond(
                &mut output,
                json!({"jsonrpc": "2.0", "id": id, "result": result}),
            )?;
            continue;
        }
        // Test documents start with an emoji and a space: UTF-16 column 3 is char 2.
        let result = match method {
            "initialize" if diagnostics => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "diagnosticProvider": {"identifier": args[1], "workspaceDiagnostics": false,
                    "interFileDependencies": args.iter().any(|arg| arg == "--inter-file")}
            }}),
            "initialize" => json!({"capabilities": {
                "positionEncoding": "utf-16", "textDocumentSync": 1,
                "documentSymbolProvider": true, "documentHighlightProvider": true,
                "colorProvider": true, "documentLinkProvider": {}
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
            "textDocument/documentLink" => json!([{
                "range": range(3, end), "target": "https://example.test/target"
            }]),
            _ => Value::Null,
        };
        respond(
            &mut output,
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        )?;
    }
}
