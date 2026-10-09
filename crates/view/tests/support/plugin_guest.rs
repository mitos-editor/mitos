//! Small real guests that validate owned event requests at the WASM boundary.
use std::path::Path;

use plugin_sdk::{Action, Response};
use plugins::PluginConfig;

#[derive(Default)]
pub(crate) struct Route<'a> {
    pub event: &'a str,
    pub response: Response,
    pub expected: Vec<String>,
    pub filter: Option<String>,
}

pub(crate) fn status(message: &str) -> Response {
    Response {
        actions: vec![Action::Status {
            message: message.into(),
        }],
        error: None,
    }
}

/// Optional ordering guard requires the named event before shutdown. Unmatched
/// events return no effects; checks are real guest code rather than host mocks.
pub(crate) fn observing(
    dir: &Path,
    subscriptions: &[&str],
    routes: &[Route<'_>],
    before_shutdown: Option<&str>,
) -> anyhow::Result<PluginConfig> {
    let mut segments = String::new();
    let mut pointer = 128_u32;
    let mut bytes = |value: &[u8]| {
        let start = pointer;
        pointer += value.len() as u32 + 1;
        let escaped = value
            .iter()
            .map(|byte| format!("\\{byte:02x}"))
            .collect::<String>();
        segments.push_str(&format!("(data (i32.const {start}) \"{escaped}\")\n"));
        (start, value.len())
    };
    let bad = serde_json::to_vec(&Response {
        actions: vec![],
        error: Some("guest event metadata or shutdown ordering mismatch".into()),
    })?;
    let (bad_pointer, bad_length) = bytes(&bad);
    let bad = (u64::from(bad_pointer) << 32) | bad_length as u64;
    let mut code = String::new();
    if before_shutdown.is_some() {
        let (ptr, len) = bytes(br#""event":"shutdown""#);
        code.push_str(&format!("local.get $input local.get $length i32.const {ptr} i32.const {len} call $contains global.get $seen i32.eqz i32.and if i64.const {bad} return end\n"));
    }
    for route in routes {
        let event = format!("\"event\":\"{}\"", route.event);
        let (ptr, len) = bytes(event.as_bytes());
        code.push_str(&format!(
            "local.get $input local.get $length i32.const {ptr} i32.const {len} call $contains\n"
        ));
        if let Some(filter) = &route.filter {
            let (ptr, len) = bytes(filter.as_bytes());
            code.push_str(&format!("local.get $input local.get $length i32.const {ptr} i32.const {len} call $contains i32.and\n"));
        }
        code.push_str("if\n");
        for expected in &route.expected {
            let (ptr, len) = bytes(expected.as_bytes());
            code.push_str(&format!("local.get $input local.get $length i32.const {ptr} i32.const {len} call $contains i32.eqz if i64.const {bad} return end\n"));
        }
        if before_shutdown == Some(route.event) {
            code.push_str("i32.const 1 global.set $seen\n");
        }
        let response = serde_json::to_vec(&route.response)?;
        let (ptr, len) = bytes(&response);
        let packed = (u64::from(ptr) << 32) | len as u64;
        code.push_str(&format!("i64.const {packed} return end\n"));
    }
    assert!(pointer < 65536, "fixture data must not overlap input");
    let wasm = wat::parse_str(format!(
        r#"(module
        (memory (export "memory") 80)
        (global $seen (mut i32) (i32.const 0))
        (data (i32.const 0) "{{}}")
        {segments}
        (func (export "mitos_alloc") (param i32) (result i32) i32.const 65536)
        (func (export "mitos_dealloc") (param i32 i32))
        (func $contains (param $input i32) (param $length i32) (param $needle i32) (param $size i32) (result i32)
            (local $offset i32) (local $index i32)
            (block $missing (loop $scan
                local.get $offset local.get $size i32.add local.get $length i32.gt_u br_if $missing
                i32.const 0 local.set $index
                (block $mismatch (loop $compare
                    local.get $index local.get $size i32.eq if i32.const 1 return end
                    local.get $input local.get $offset i32.add local.get $index i32.add i32.load8_u
                    local.get $needle local.get $index i32.add i32.load8_u i32.ne br_if $mismatch
                    local.get $index i32.const 1 i32.add local.set $index br $compare))
                local.get $offset i32.const 1 i32.add local.set $offset br $scan))
            i32.const 0)
        (func (export "mitos_call") (param $input i32) (param $length i32) (result i64)
            {code}
            i64.const 2))"#
    ))?;
    std::fs::write(dir.join("plugin.wasm"), wasm)?;
    std::fs::write(dir.join("plugin.toml"), format!("abi-version = {}\nmodule = 'plugin.wasm'\nevents = {subscriptions:?}\n[commands.run]\ndoc = 'Observed guest'\n", plugin_sdk::ABI_VERSION))?;
    Ok(PluginConfig {
        path: dir.join("plugin.toml"),
        enabled: true,
        config: serde_json::Value::Null,
    })
}
