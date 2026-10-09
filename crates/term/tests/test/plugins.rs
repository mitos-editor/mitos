use super::helpers::{test_config, test_key_sequences, AppBuilder};

use editor_core::diagnostic::Severity;
use plugins::PluginConfig;
use serde_json::{json, Value};
use tempfile::TempDir;
use term::config::Config;
use view::custom_commands::{CustomCommand, CustomCommands};

/// A real guest with persistent memory, an empty initialization response, and
/// optional request checking to observe the editor's argument parsing.
fn fixture(response: Value, expected_args: Option<&[&str]>) -> anyhow::Result<(TempDir, Config)> {
    fn wat_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
    }

    let response = serde_json::to_vec(&response)?;
    let expected = expected_args
        .map(|args| serde_json::to_string(args).map(|args| format!("\"args\":{args}")))
        .transpose()?
        .unwrap_or_default();
    let wrong_args = br#"{"error":"unexpected command arguments"}"#;
    let command_event = br#""event":"command""#;
    let wasm = wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (global $initialized (mut i32) (i32.const 0))
            (data (i32.const 0) "{{}}")
            (data (i32.const 4096) "{}")
            (data (i32.const 8192) "{}")
            (data (i32.const 12288) "{}")
            (data (i32.const 16384) "{}")
            (func (export "mitos_alloc") (param i32) (result i32)
                i32.const 32768)
            (func (export "mitos_dealloc") (param i32 i32))
            (func $contains (param $input i32) (param $length i32)
                            (param $needle i32) (param $needle_length i32) (result i32)
                (local $offset i32) (local $index i32)
                (block $missing
                    (loop $scan
                        local.get $offset
                        local.get $needle_length
                        i32.add
                        local.get $length
                        i32.gt_u
                        br_if $missing
                        i32.const 0
                        local.set $index
                        (block $mismatch
                            (loop $compare
                                local.get $index
                                local.get $needle_length
                                i32.eq
                                if
                                    i32.const 1
                                    return
                                end
                                local.get $input
                                local.get $offset
                                i32.add
                                local.get $index
                                i32.add
                                i32.load8_u
                                local.get $needle
                                local.get $index
                                i32.add
                                i32.load8_u
                                i32.ne
                                br_if $mismatch
                                local.get $index
                                i32.const 1
                                i32.add
                                local.set $index
                                br $compare))
                        local.get $offset
                        i32.const 1
                        i32.add
                        local.set $offset
                        br $scan))
                i32.const 0)
            (func (export "mitos_call") (param $input i32) (param $length i32) (result i64)
                global.get $initialized
                i32.eqz
                if (result i64)
                    i32.const 1
                    global.set $initialized
                    i64.const 2
                else
                    local.get $input
                    local.get $length
                    i32.const 16384
                    i32.const {command_event_len}
                    call $contains
                    if (result i64)
                        local.get $input
                        local.get $length
                        i32.const 8192
                        i32.const {expected_len}
                        call $contains
                        if (result i64)
                            i64.const {response_buffer}
                        else
                            i64.const {wrong_args_buffer}
                        end
                    else
                        i64.const 2
                    end
                end))"#,
        wat_bytes(&response),
        wat_bytes(expected.as_bytes()),
        wat_bytes(wrong_args),
        wat_bytes(command_event),
        command_event_len = command_event.len(),
        expected_len = expected.len(),
        response_buffer = (4096_u64 << 32) | response.len() as u64,
        wrong_args_buffer = (12288_u64 << 32) | wrong_args.len() as u64,
    ))?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("fixture.wasm"), wasm)?;
    std::fs::write(
        dir.path().join("plugin.toml"),
        "abi-version = 1\nmodule = 'fixture.wasm'\n[commands.run]\ndoc = 'Run the fixture guest'\n",
    )?;
    let mut config = test_config();
    config.plugins.insert(
        "fixture".into(),
        PluginConfig {
            path: dir.path().into(),
            enabled: true,
            config: Value::Null,
        },
    );
    Ok((dir, config))
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_commands_work_in_prompt_keybindings_custom_commands_and_palette(
) -> anyhow::Result<()> {
    let (_dir, mut config) = fixture(
        json!({"actions": [{"type": "status", "message": "Wasm command ran"}]}),
        None,
    )?;
    config.keys = toml::from_str("[normal]\nF11 = ':echo builtin'\nF12 = ':fixture.run'\n")?;
    config.editor.commands = CustomCommands::new(vec![
        CustomCommand {
            commands: vec![":fixture.run".into()],
            ..CustomCommand::default()
        }
        .named(":hello".into()),
        CustomCommand {
            commands: vec![":fixture.run".into()],
            ..CustomCommand::default()
        }
        .named(":echo".into()),
    ]);
    let mut app = AppBuilder::new().with_config(config).build()?;
    let commands = app.editor.plugin_commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "fixture.run");
    assert_eq!(commands[0].doc, "Run the fixture guest");
    assert_eq!(
        app.editor.plugin_command_doc("fixture.run").as_deref(),
        Some("Run the fixture guest")
    );
    assert!(!app.editor.is_err());

    let ran = |app: &term::application::Application| {
        let (message, severity) = app.editor.get_status().unwrap();
        assert_eq!(message.as_ref(), "Wasm command ran");
        assert_eq!(*severity, Severity::Info);
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some(":fixture.run<ret>"), Some(&ran)),
            (Some("<F12>"), Some(&ran)),
            (Some(":hello<ret>"), Some(&ran)),
            (Some(":echo<ret>"), Some(&ran)),
            (
                Some("<F11>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "builtin");
                }),
            ),
            (Some(":fixture.ru<tab><ret>"), Some(&ran)),
            (Some("<space>?fixture.run<ret>"), Some(&ran)),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_arguments_preserve_quotes_and_editor_and_custom_expansions() -> anyhow::Result<()> {
    let (_dir, mut config) = fixture(
        json!({"actions": [{"type": "status", "message": "Arguments matched"}]}),
        Some(&["hello world", "line1"]),
    )?;
    config.editor.commands = CustomCommands::new(vec![CustomCommand {
        commands: vec![":fixture.run %arg{0} \"line%{cursor_line}\"".into()],
        ..CustomCommand::default()
    }
    .named(":hello".into())]);
    let mut app = AppBuilder::new().with_config(config).build()?;
    let matched = |app: &term::application::Application| {
        let (message, severity) = app.editor.get_status().unwrap();
        assert_eq!(message.as_ref(), "Arguments matched");
        assert_eq!(*severity, Severity::Info);
    };
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(r#":fixture.run 'hello world' "line%{cursor_line}"<ret>"#),
                Some(&matched),
            ),
            (Some(":hello 'hello world'<ret>"), Some(&matched)),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_plugin_commands_and_guest_errors_are_visible() -> anyhow::Result<()> {
    let (_dir, config) = fixture(json!({"error": "fixture rejected command"}), None)?;
    let mut app = AppBuilder::new().with_config(config).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":missing.run<ret>"),
                Some(&|app| {
                    let (message, severity) = app.editor.get_status().unwrap();
                    assert_eq!(message.as_ref(), "no such command: 'missing.run'");
                    assert_eq!(*severity, Severity::Error);
                }),
            ),
            (
                Some(":fixture.run<ret>"),
                Some(&|app| {
                    let (message, severity) = app.editor.get_status().unwrap();
                    assert!(message.contains("fixture rejected command"), "{message}");
                    assert_eq!(*severity, Severity::Error);
                }),
            ),
        ],
        false,
    )
    .await
}
