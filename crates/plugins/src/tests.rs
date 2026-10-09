use std::fs;

use plugin_api::Action;
use serde_json::json;
use tempfile::TempDir;

use super::*;

const MANIFEST: &str = r#"
abi-version = 2
module = "plugin.wasm"
capabilities = ["ui", "editor-read"]
events = ["document-opened"]
[commands.run]
doc = "Run the example command"
"#;
const REQUEST_POINTER: usize = 32768;

fn fixture(manifest: &str, module: &str) -> (TempDir, PluginConfig) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("plugin.toml"), manifest).unwrap();
    fs::write(
        directory.path().join("plugin.wasm"),
        wat::parse_str(module).unwrap(),
    )
    .unwrap();
    let config = PluginConfig {
        path: directory.path().join("plugin.toml"),
        enabled: true,
        config: json!({ "prefix": "example" }),
        permissions: Permissions {
            capabilities: [Capability::Ui, Capability::EditorRead].into(),
            ..Permissions::default()
        },
        ..PluginConfig::default()
    };
    (directory, config)
}

fn module(response: &str, body: &str) -> String {
    let data: String = response
        .bytes()
        .map(|byte| format!("\\{byte:02x}"))
        .collect();
    format!(
        r#"(module
            (memory (export "memory") 1)
            (global $calls (mut i32) (i32.const 0))
            (data (i32.const 16) "{data}")
            (func (export "mitos_alloc") (param i32) (result i32) i32.const {REQUEST_POINTER})
            (func (export "mitos_dealloc") (param i32 i32))
            (func (export "mitos_call") (param $ptr i32) (param $len i32) (result i64)
                {body}
                i64.const {}
            )
        )"#,
        (16_u64 << 32) | response.len() as u64
    )
}

fn load_manager(config: PluginConfig) -> PluginManager {
    let (manager, errors) = PluginManager::load(
        &BTreeMap::from([("example".into(), config)]),
        Path::new("."),
    );
    assert!(errors.is_empty(), "{errors:?}");
    manager
}

fn command(manager: &mut PluginManager) -> Result<Option<Response>> {
    manager.call_command("example.run", vec![], EditorContext::default())
}

#[test]
fn module_pins_and_permission_revocation_reject_replacements_and_calls() {
    let (_directory, mut config) = fixture(MANIFEST, &module("{}", ""));
    config.sha256 = Some("0".repeat(64));
    let (manager, errors) = PluginManager::load(
        &BTreeMap::from([("example".into(), config)]),
        Path::new("."),
    );
    assert!(manager.available_commands().is_empty());
    assert!(errors[0].contains("SHA-256"));

    let (_directory, config) = fixture(MANIFEST, &module("{}", ""));
    let mut manager = load_manager(config);
    manager.policy("example").unwrap().revoke();
    assert!(command(&mut manager).is_err());
    assert!(!manager.subscribes(Event::DocumentOpened));
}

#[test]
fn document_observation_requires_declaration_and_user_grant() {
    let (_directory, mut config) = fixture(MANIFEST, &module("{}", ""));
    config.permissions = Permissions::default();
    let mut manager = load_manager(config);
    assert!(!manager.subscribes(Event::DocumentOpened));
    assert!(manager
        .dispatch_event(Event::DocumentOpened, EditorContext::default(), Value::Null)
        .is_empty());
    assert!(manager.subscribes(Event::Init));
    assert!(manager.subscribes(Event::UiResult));
}

#[test]
fn commands_are_documented_and_receive_their_request() {
    let (_directory, config) = fixture(MANIFEST, &module("{}", ""));
    let mut manager = load_manager(config);
    assert_eq!(
        manager.available_commands(),
        vec![PluginCommand {
            name: "example.run".into(),
            doc: "Run the example command".into(),
        }]
    );
    assert_eq!(
        manager.get_doc_for_identifier("example.run"),
        Some("Run the example command".into())
    );
    assert!(manager
        .call_command("unknown.run", vec![], EditorContext::default())
        .unwrap()
        .is_none());
    assert!(manager
        .call_command("example.missing", vec![], EditorContext::default())
        .unwrap()
        .is_none());

    let editor = EditorContext {
        mode: "insert".into(),
        document: None,
        ..EditorContext::default()
    };
    let args = vec!["argument".into()];
    assert_eq!(
        manager
            .call_command("example.run", args.clone(), editor.clone())
            .unwrap(),
        Some(Response::default())
    );

    let expected = Request {
        abi_version: ABI_VERSION,
        event: Event::Command,
        command: Some("run".into()),
        args,
        config: json!({ "prefix": "example" }),
        editor,
        data: Value::Null,
    };
    let length = serde_json::to_vec(&expected).unwrap().len();
    let guest = manager.plugins["example"].instance.as_ref().unwrap();
    let request: Request = serde_json::from_slice(
        &guest.memory.data(&guest.store)[REQUEST_POINTER..REQUEST_POINTER + length],
    )
    .unwrap();
    assert_eq!(request, expected);
}

#[test]
fn guest_state_persists_and_only_subscribed_events_run() {
    let response = r#"{"actions":[{"type":"status","message":"0"}]}"#;
    let position = 16 + response.find("\"0\"").unwrap() + 1;
    let body = format!(
        r#"
        global.get $calls
        i32.const 1
        i32.add
        global.set $calls
        i32.const {position}
        global.get $calls
        i32.const 48
        i32.add
        i32.store8
    "#
    );
    let (_directory, config) = fixture(MANIFEST, &module(response, &body));
    let mut manager = load_manager(config);
    assert!(manager.subscribes(Event::Init));
    assert!(manager.subscribes(Event::Shutdown));
    assert!(manager.subscribes(Event::DocumentOpened));
    assert!(!manager.subscribes(Event::DocumentChanged));
    assert!(manager
        .dispatch_event(
            Event::DocumentChanged,
            EditorContext::default(),
            Value::Null
        )
        .is_empty());
    let init = manager.dispatch_event(Event::Init, EditorContext::default(), Value::Null);
    assert_eq!(
        init[0].1.as_ref().unwrap().actions,
        vec![Action::Status {
            message: "1".into()
        }]
    );
    assert_eq!(
        command(&mut manager).unwrap().unwrap().actions,
        vec![Action::Status {
            message: "2".into()
        }]
    );
    let shutdown = manager.dispatch_event(Event::Shutdown, EditorContext::default(), Value::Null);
    assert_eq!(
        shutdown[0].1.as_ref().unwrap().actions,
        vec![Action::Status {
            message: "3".into()
        }]
    );
}

#[test]
fn trapping_plugin_is_disabled_without_affecting_other_plugins() {
    let (_bad_directory, bad) = fixture(MANIFEST, &module("{}", "(loop $again br $again)"));
    let (_good_directory, good) = fixture(MANIFEST, &module("{}", ""));
    let (mut manager, errors) = PluginManager::load(
        &BTreeMap::from([("bad".into(), bad), ("good".into(), good)]),
        Path::new("."),
    );
    assert!(errors.is_empty());
    assert!(manager
        .call_command("bad.run", vec![], EditorContext::default())
        .is_err());
    assert!(manager.plugins["bad"].instance.is_none());
    assert!(manager
        .call_command("bad.run", vec![], EditorContext::default())
        .unwrap_err()
        .to_string()
        .contains("plugin 'bad'"));
    assert_eq!(
        manager
            .call_command("good.run", vec![], EditorContext::default())
            .unwrap(),
        Some(Response::default())
    );
    assert!(manager.get_doc_for_identifier("bad.run").is_some());
}

#[test]
fn allocation_and_deallocation_are_also_metered() {
    let wasm = module("{}", "");
    let allocation = wasm.replace(
        &format!("(result i32) i32.const {REQUEST_POINTER}"),
        &format!("(result i32) (loop $again br $again) i32.const {REQUEST_POINTER}"),
    );
    let deallocation = wasm.replace(
        "(func (export \"mitos_dealloc\") (param i32 i32))",
        "(func (export \"mitos_dealloc\") (param i32 i32) (loop $again br $again))",
    );
    for wasm in [allocation, deallocation] {
        let (_directory, config) = fixture(MANIFEST, &wasm);
        let mut manager = load_manager(config);
        assert!(command(&mut manager).is_err());
        assert!(manager.plugins["example"].instance.is_none());
    }
}

#[test]
fn invalid_guest_buffers_and_json_disable_the_instance() {
    for body in [
        format!("(return (i64.const {}))", (u64::from(u32::MAX) << 32) | 2),
        format!(
            "(return (i64.const {}))",
            (16_u64 << 32) | (MAX_MESSAGE_BYTES as u64 + 1)
        ),
        format!(
            "(return (i64.const {}))",
            ((REQUEST_POINTER as u64) << 32) | 2
        ),
    ] {
        let (_directory, config) = fixture(MANIFEST, &module("{}", &body));
        let mut manager = load_manager(config);
        assert!(command(&mut manager).is_err());
        assert!(manager.plugins["example"].instance.is_none());
    }
    let (_directory, config) = fixture(MANIFEST, &module("not JSON", ""));
    let mut manager = load_manager(config);
    assert!(command(&mut manager).is_err());
    assert!(manager.plugins["example"].instance.is_none());
}

#[test]
fn oversized_host_requests_and_declared_errors_keep_the_plugin_active() {
    let (_directory, config) = fixture(MANIFEST, &module(r#"{"error":"command failed"}"#, ""));
    let mut manager = load_manager(config);
    assert!(manager
        .call_command(
            "example.run",
            vec!["x".repeat(MAX_MESSAGE_BYTES)],
            EditorContext::default()
        )
        .is_err());
    assert!(manager.plugins["example"].instance.is_some());
    assert_eq!(
        command(&mut manager).unwrap().unwrap().error,
        Some("command failed".into())
    );
    assert!(manager.plugins["example"].instance.is_some());
}

#[test]
fn memory_growth_and_action_count_are_limited() {
    let (_directory, config) = fixture(MANIFEST, &module("{}", "i32.const 1024 memory.grow drop"));
    let mut manager = load_manager(config);
    assert!(command(&mut manager).is_err());
    assert!(manager.plugins["example"].instance.is_none());

    let response = serde_json::to_string(&Response {
        actions: vec![
            Action::Status {
                message: String::new()
            };
            MAX_ACTIONS + 1
        ],
        error: None,
    })
    .unwrap();
    let (_directory, config) = fixture(MANIFEST, &module(&response, ""));
    let mut manager = load_manager(config);
    let error = command(&mut manager).unwrap_err();
    assert!(format!("{error:#}").contains("too many plugin actions"));
}

#[test]
fn manifest_and_module_load_failures_are_isolated() {
    for invalid in [
        MANIFEST.replace("abi-version = 2", "abi-version = 1"),
        MANIFEST.replace("abi-version = 2", "unknown-key = 1\nabi-version = 2"),
        MANIFEST.replace("plugin.wasm", "../plugin.wasm"),
        MANIFEST.replace("commands.run", "commands.'run.with.dots'"),
    ] {
        let (_directory, config) = fixture(&invalid, &module("{}", ""));
        let (manager, errors) = PluginManager::load(
            &BTreeMap::from([("example".into(), config)]),
            Path::new("."),
        );
        assert_eq!(errors.len(), 1);
        assert!(manager.available_commands().is_empty());
    }
    for invalid in [
        "(module (memory (export \"memory\") 1))",
        "(module (import \"wasi_snapshot_preview1\" \"fd_write\" (func)))",
        "(module (memory (export \"memory\") 1025))",
        "(module (table 4097 funcref))",
        "(module (func $start (loop $again br $again)) (start $start))",
    ] {
        let (_bad_directory, bad) = fixture(MANIFEST, invalid);
        let (_good_directory, good) = fixture(MANIFEST, &module("{}", ""));
        let (manager, errors) = PluginManager::load(
            &BTreeMap::from([("bad".into(), bad), ("good".into(), good)]),
            Path::new("."),
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(manager.available_commands()[0].name, "good.run");
    }
}

#[test]
fn configuration_defaults_and_relative_manifest_paths_work() {
    let (_directory, config) = fixture(MANIFEST, &module("{}", ""));
    let mut config = config;
    let base = config.path.parent().unwrap().to_owned();
    config.path = "plugin.toml".into();
    let (manager, errors) =
        PluginManager::load(&BTreeMap::from([("example".into(), config.clone())]), &base);
    assert!(errors.is_empty());
    assert_eq!(manager.available_commands()[0].name, "example.run");
    config.enabled = false;
    config.path = "missing.toml".into();
    let (manager, errors) =
        PluginManager::load(&BTreeMap::from([("example".into(), config)]), &base);
    assert!(errors.is_empty());
    assert!(manager.available_commands().is_empty());
    let config: PluginConfig = toml::from_str("path = 'plugin.toml'").unwrap();
    assert!(config.enabled);
    assert_eq!(config.config, Value::Null);
}
