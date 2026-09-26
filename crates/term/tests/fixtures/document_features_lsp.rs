//! Minimal stdio language server for document-feature integration tests.
//! Full text sync and UTF-16 positions exercise the real client transport.
use std::{
    collections::HashMap,
    io::{self, BufRead, Read, Write},
};

use serde_json::{json, Value};

fn range(start: usize, end: usize) -> Value {
    json!({"start": {"line": 0, "character": start}, "end": {"line": 0, "character": end}})
}

fn main() -> anyhow::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut documents = HashMap::<String, String>::new();
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
        match method {
            "textDocument/didOpen" => {
                documents.insert(
                    uri.into(),
                    params["textDocument"]["text"].as_str().unwrap().into(),
                );
                continue;
            }
            "textDocument/didChange" => {
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
        // Test documents start with an emoji and a space: UTF-16 column 3 is char 2.
        let result = match method {
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
        let body = serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": id, "result": result}))?;
        write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
        output.write_all(&body)?;
        output.flush()?;
    }
}
