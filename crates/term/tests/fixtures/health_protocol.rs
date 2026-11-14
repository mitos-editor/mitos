//! Small protocol peer for the health command's process tests.
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
};

fn send(output: &mut impl Write, value: Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(&value)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    mode: &str,
    log: &str,
) -> anyhow::Result<()> {
    std::fs::write(
        std::path::Path::new(log).with_extension("pid"),
        std::process::id().to_string(),
    )?;
    let mut log = OpenOptions::new().create(true).append(true).open(log)?;
    let mut initialize = Value::Null;
    loop {
        let mut length = None;
        loop {
            let mut header = String::new();
            if input.read_line(&mut header)? == 0 {
                return Ok(());
            }
            if header.trim().is_empty() {
                break;
            }
            if let Some(value) = header.strip_prefix("Content-Length: ") {
                length = Some(value.trim().parse::<usize>()?);
            }
        }
        let mut body = vec![0; length.ok_or_else(|| anyhow::anyhow!("missing length"))?];
        input.read_exact(&mut body)?;
        let message: Value = serde_json::from_slice(&body)?;
        let method = message["method"]
            .as_str()
            .or_else(|| message["command"].as_str())
            .unwrap_or("response");
        writeln!(log, "{method}")?;
        log.flush()?;
        if mode == "hang" {
            continue;
        }
        if mode == "fail" {
            return Ok(());
        }
        if mode == "lsp" {
            match method {
                "initialize" => {
                    anyhow::ensure!(std::env::var("MITOS_HEALTH_FIXTURE")?.as_str() == "present");
                    anyhow::ensure!(
                        message["params"]["rootPath"] == std::env::current_dir()?.to_str().unwrap()
                    );
                    anyhow::ensure!(
                        message["params"]["initializationOptions"]["health"] == "configured"
                    );
                    send(
                        &mut output,
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"capabilities":{"hoverProvider":true,"completionProvider":{},"renameProvider":false,"documentFormattingProvider":{},"foldingRangeProvider":true}}}),
                    )?;
                }
                "shutdown" => send(
                    &mut output,
                    json!({"jsonrpc":"2.0","id":message["id"],"result":null}),
                )?,
                "exit" => return Ok(()),
                "initialized" => {}
                _ => anyhow::bail!("unexpected health request: {method}"),
            }
        } else {
            if message["type"] == "response" {
                anyhow::ensure!(message["success"] == false);
                send(
                    &mut output,
                    json!({"seq":2,"type":"response","command":"initialize","request_seq":initialize,"success":true,"body":{"supportsConditionalBreakpoints":true,"supportsLogPoints":true,"supportsStepBack":false}}),
                )?;
                continue;
            }
            match method {
                "initialize" => {
                    anyhow::ensure!(message["arguments"]["adapterID"] == "health-dap");
                    anyhow::ensure!(message["arguments"]["supportsRunInTerminalRequest"] == false);
                    anyhow::ensure!(message["arguments"]["supportsStartDebuggingRequest"] == false);
                    initialize = message["seq"].clone();
                    send(
                        &mut output,
                        json!({"seq":1,"type":"request","command":"runInTerminal","arguments":{}}),
                    )?;
                }
                "disconnect" => {
                    send(
                        &mut output,
                        json!({"seq":3,"type":"response","command":"disconnect","request_seq":message["seq"],"success":true,"body":{}}),
                    )?;
                    return Ok(());
                }
                _ => anyhow::bail!("unexpected health request: {method}"),
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args[1] == "tcp" {
        let port = args[3].strip_prefix("--port=").unwrap().parse::<u16>()?;
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let (stream, _) = listener.accept()?;
        serve(BufReader::new(stream.try_clone()?), stream, "dap", &args[2])
    } else {
        serve(io::stdin().lock(), io::stdout().lock(), &args[1], &args[2])
    }
}
