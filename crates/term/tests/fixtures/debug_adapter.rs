//! Controlled stdio/TCP adapter exercising startup and child-session handshakes.
#[path = "../../../dap/tests/support/adapter.rs"]
mod adapter_fixture;

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn send(output: &mut impl Write, sequence: &mut u64, message: Value) -> anyhow::Result<()> {
    output.write_all(&adapter_fixture::frame(sequence, message)?)?;
    output.flush()?;
    Ok(())
}

fn serve(mut input: impl BufRead, mut output: impl Write) -> anyhow::Result<()> {
    let mut sequence = 1;
    let variables = adapter_fixture::VariableFixture::new([1, 2, 3], Some((4, 250)));
    let mut gate = adapter_fixture::RequestGate::default();
    let mut hold_variables = false;
    let mut fail_variables = false;
    let mut child_request = None;
    let mut stack_count = 1;
    let mut adapter_source = false;
    let mut source_path = Value::Null;
    let mut nested_value = "nested value";
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
        if command == "launch" {
            hold_variables = message["arguments"]["holdVariables"] == true;
            fail_variables = message["arguments"]["failVariables"] == true;
            source_path = message["arguments"]["sourcePath"].clone();
        }
        if hold_variables && command == "variables" {
            hold_variables = false;
            gate.hold(message);
            continue;
        }
        let body = match command {
            "initialize" => {
                anyhow::ensure!(
                    message["arguments"]["supportsStartDebuggingRequest"] == true,
                    "child-session support was not advertised"
                );
                json!({"supportsConfigurationDoneRequest":true,"supportsDelayedStackTraceLoading":true,"supportsEvaluateForHovers":true})
            }
            "evaluate" if message["arguments"]["expression"] == "answer + 1" => {
                anyhow::ensure!(message["arguments"]["context"] == "hover");
                json!({"result":"43","variablesReference":0})
            }
            "evaluate" if message["arguments"]["expression"] == "object" => {
                json!({"result":"Object","variablesReference":4,"namedVariables":250,"indexedVariables":0})
            }
            "evaluate" => {
                nested_value = "updated value";
                json!({"result":"updated","variablesReference":0})
            }
            "threads" => json!({"threads":[{"id":1,"name":"main"}]}),
            "stackTrace" => {
                let start = message["arguments"]["startFrame"].as_u64().unwrap_or(0) as usize;
                let levels = message["arguments"]["levels"]
                    .as_u64()
                    .unwrap_or(stack_count as u64) as usize;
                json!({"stackFrames":(start..stack_count.min(start.saturating_add(levels))).map(|i| json!({"id":i+1,"name":format!("frame{i}"),"line":1,"column":1,"source":if adapter_source {json!({"name":"generated.rs","sourceReference":7})} else if !source_path.is_null() {json!({"path":source_path})} else {Value::Null}})).collect::<Vec<_>>(),"totalFrames":stack_count})
            }
            "setBreakpoints" => adapter_fixture::default_response(&message),
            "source" => json!({"content":"adapter content\n"}),
            "launch" if message["arguments"]["inspectSource"] == true => {
                adapter_source = true;
                json!({})
            }
            "launch" if message["arguments"]["inspectStack"] == true => {
                stack_count = 70;
                json!({})
            }

            "scopes" => variables.scopes(),
            "variables" => variables.variables(&message["arguments"], nested_value)?,
            _ => adapter_fixture::default_response(&message),
        };
        send(
            &mut output,
            &mut sequence,
            adapter_fixture::response(&message, !(fail_variables && command == "variables"), body),
        )?;
        if command == "continue"
            && let Some(held) = gate.release(Some("variables"))
        {
            let body = variables.variables(&held["arguments"], nested_value)?;
            send(
                &mut output,
                &mut sequence,
                adapter_fixture::response(&held, true, body),
            )?;
        }
        match command {
            "initialize" => send(
                &mut output,
                &mut sequence,
                json!({"type":"event","event":"initialized"}),
            )?,
            "launch"
                if message["arguments"]["inspectVariables"] == true
                    || message["arguments"]["inspectStack"] == true
                    || message["arguments"]["inspectSource"] == true =>
            {
                send(
                    &mut output,
                    &mut sequence,
                    json!({"type":"event","event":"stopped","body":{"reason":"breakpoint","threadId":1,"allThreadsStopped":true}}),
                )?
            }
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
