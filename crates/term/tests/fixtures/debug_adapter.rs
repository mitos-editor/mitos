//! Controlled stdio/TCP adapter exercising startup and child-session handshakes.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn send(output: &mut impl Write, sequence: &mut u64, mut message: Value) -> anyhow::Result<()> {
    message["seq"] = (*sequence).into();
    *sequence += 1;
    let body = serde_json::to_vec(&message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

fn serve(mut input: impl BufRead, mut output: impl Write) -> anyhow::Result<()> {
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
        let body = match command {
            "initialize" => {
                anyhow::ensure!(
                    message["arguments"]["supportsStartDebuggingRequest"] == true,
                    "child-session support was not advertised"
                );
                json!({"supportsConfigurationDoneRequest":true})
            }
            "threads" => json!({"threads":[{"id":1,"name":"main"}]}),
            "stackTrace" => json!({"stackFrames":[{"id":1,"name":"main","line":1,"column":1}]}),
            "scopes" => {
                json!({"scopes":[{"name":"locals","variablesReference":1,"expensive":false},{"name":"globals","variablesReference":2,"expensive":true}]})
            }
            "variables" => match message["arguments"]["variablesReference"].as_u64() {
                Some(1) => {
                    json!({"variables":[{"name":"answer","value":"42","variablesReference":0},{"name":"object","value":"Object","variablesReference":3}]})
                }
                Some(2) => {
                    json!({"variables":[{"name":"global","value":"true","variablesReference":0}]})
                }
                Some(3) => {
                    json!({"variables":[{"name":"child","value":"nested value","variablesReference":0}]})
                }
                reference => anyhow::bail!("unexpected variable reference: {reference:?}"),
            },
            _ => json!({}),
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
            "launch" if message["arguments"]["inspectVariables"] == true => send(
                &mut output,
                &mut sequence,
                json!({"type":"event","event":"stopped","body":{"reason":"breakpoint","threadId":1,"allThreadsStopped":true}}),
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

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut port = None;
    let mut gate = None;
    let mut started = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit" => std::process::exit(23),
            "--tcp" => port = Some(args.next().unwrap().parse::<u16>()?),
            "--gate" => gate = Some(std::path::PathBuf::from(args.next().unwrap())),
            "--started" => started = Some(std::path::PathBuf::from(args.next().unwrap())),
            _ => anyhow::bail!("unknown fixture argument: {arg}"),
        }
    }
    let Some(port) = port else {
        return serve(io::stdin().lock(), io::stdout().lock());
    };
    // A probe listener makes process cleanup observable without platform-specific PID APIs.
    let probe = std::net::TcpListener::bind("127.0.0.1:0")?;
    if let Some(path) = started {
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, probe.local_addr()?.to_string())?;
        std::fs::rename(temporary, path)?;
    }
    if let Some(gate) = gate {
        while !gate.exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
    let (stream, _) = listener.accept()?;
    let result = serve(io::BufReader::new(stream.try_clone()?), stream);
    drop(probe);
    result
}
