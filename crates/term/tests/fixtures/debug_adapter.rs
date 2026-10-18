//! Minimal stdio adapter exercising parent/child reverse-request handshakes.
use serde_json::{json, Value};
use std::io::{self, BufRead, Read, Write};

fn send(output: &mut impl Write, sequence: &mut u64, mut message: Value) -> anyhow::Result<()> {
    message["seq"] = (*sequence).into();
    *sequence += 1;
    let body = serde_json::to_vec(&message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut sequence = 1;
    let mut child_request = None;
    loop {
        let mut header = String::new();
        if input.read_line(&mut header)? == 0 {
            return Ok(());
        }
        let length: usize = header
            .trim()
            .strip_prefix("Content-Length: ")
            .ok_or_else(|| anyhow::anyhow!("missing Content-Length"))?
            .parse()?;
        header.clear();
        input.read_line(&mut header)?;
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        let message: Value = serde_json::from_slice(&bytes)?;
        if message["type"] == "response" {
            if child_request == message["request_seq"].as_u64() {
                anyhow::ensure!(
                    message["success"] == true,
                    "child startup failed: {message}"
                );
                send(
                    &mut output,
                    &mut sequence,
                    json!({"type":"event","event":"output","body":{"output":"stdio child started"}}),
                )?;
            }
            continue;
        }
        let command = message["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing command"))?;
        let body = if command == "initialize" {
            anyhow::ensure!(
                message["arguments"]["supportsStartDebuggingRequest"] == true,
                "child-session support was not advertised"
            );
            json!({"supportsConfigurationDoneRequest":true})
        } else {
            json!({})
        };
        send(
            &mut output,
            &mut sequence,
            json!({"type":"response","request_seq":message["seq"],"command":command,"success":true,"body":body}),
        )?;
        match command {
            "initialize" => send(
                &mut output,
                &mut sequence,
                json!({"type":"event","event":"initialized"}),
            )?,
            "launch" if message["arguments"]["spawnChild"] == true => {
                child_request = Some(sequence);
                send(
                    &mut output,
                    &mut sequence,
                    json!({"type":"request","command":"startDebugging","arguments":{"request":"launch","configuration":{"child":true}}}),
                )?;
            }
            "disconnect" => return Ok(()),
            _ => {}
        }
    }
}
