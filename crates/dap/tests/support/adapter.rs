//! Protocol fixtures shared by the headless TCP adapter and the terminal's stdio adapter.
use serde_json::{json, Value};

/// Encode complete DAP frames with monotonic sequence numbers and byte lengths.
pub fn frame(sequence: &mut u64, mut message: Value) -> anyhow::Result<Vec<u8>> {
    message["seq"] = (*sequence).into();
    *sequence += 1;
    let body = serde_json::to_vec(&message)?;
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body);
    Ok(frame)
}

pub fn response(request: &Value, success: bool, body: Value) -> Value {
    let mut reply = json!({"type":"response","request_seq":request["seq"],"command":request["command"],"success":success,"body":body});
    if !success {
        reply["message"] = "fixture failure".into();
        reply["body"] = json!({"error":{"id":1,"format":"fixture failure"}});
    }
    reply
}

/// Common acknowledgements and breakpoint verification for both transports.
pub fn default_response(request: &Value) -> Value {
    if request["command"] != "setBreakpoints" {
        return json!({});
    }
    let breakpoints: Vec<_> = request["arguments"]["breakpoints"].as_array().into_iter().flatten().map(|breakpoint| {
        json!({"id":1000 + breakpoint["line"].as_u64().unwrap(),"verified":true,"line":breakpoint["line"]})
    }).collect();
    json!({"breakpoints":breakpoints})
}

/// Hold replies until the test explicitly releases them or advances the adapter.
/// Named release allows several commands to be held without losing older requests.
#[derive(Default)]
pub struct RequestGate {
    pending: Vec<Value>,
}

impl RequestGate {
    pub fn hold(&mut self, request: Value) {
        self.pending.push(request);
    }
    pub fn release(&mut self, command: Option<&str>) -> Option<Value> {
        let index = match command {
            Some(command) => self
                .pending
                .iter()
                .rposition(|request| request["command"] == command)?,
            None => self.pending.len().checked_sub(1)?,
        };
        Some(self.pending.remove(index))
    }
}

/// Shared scopes and nested values; each transport chooses its reference namespace.
pub struct VariableFixture {
    references: [usize; 3],
    large: Option<(usize, usize)>,
    pub count: Option<usize>,
}

impl VariableFixture {
    pub fn new(references: [usize; 3], large: Option<(usize, usize)>) -> Self {
        Self {
            references,
            large,
            count: None,
        }
    }

    pub fn scopes(&self) -> Value {
        let [locals, globals, _] = self.references;
        let mut scopes = vec![
            json!({"name":"locals","variablesReference":locals,"expensive":false}),
            json!({"name":"globals","variablesReference":globals,"expensive":true}),
        ];
        if let Some((reference, _)) = self.large {
            scopes.push(json!({"name":"large","variablesReference":reference,"expensive":false}));
        }
        json!({"scopes":scopes})
    }

    pub fn variables(&self, arguments: &Value, nested_value: &str) -> anyhow::Result<Value> {
        let [locals, globals, object] = self.references;
        let reference = arguments["variablesReference"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing variable reference"))?
            as usize;
        if reference == locals {
            if let Some(total) = self.count {
                return Ok(self.range(arguments, total, object));
            }
            return Ok(
                json!({"variables":[{"name":"answer","value":"42","type":"int","variablesReference":0},{"name":"object","value":"Object","variablesReference":object}]}),
            );
        }
        if reference == globals {
            return Ok(
                json!({"variables":[{"name":"global","value":"true","variablesReference":0}]}),
            );
        }
        if reference == object {
            return Ok(
                json!({"variables":[{"name":"child","value":nested_value,"variablesReference":0}]}),
            );
        }
        if let Some((large, total)) = self.large
            && reference == large
        {
            return Ok(self.range(arguments, total, 0));
        }
        anyhow::bail!("unknown variable reference {reference}")
    }

    fn range(&self, arguments: &Value, total: usize, children: usize) -> Value {
        let start = arguments["start"].as_u64().unwrap_or(0) as usize;
        let count = arguments["count"]
            .as_u64()
            .filter(|count| *count > 0)
            .map_or(total, |count| count as usize);
        json!({"variables":(start..total.min(start.saturating_add(count))).map(|index| json!({"name":format!("item{index}"),"value":index.to_string(),"variablesReference":children})).collect::<Vec<_>>()})
    }
}
