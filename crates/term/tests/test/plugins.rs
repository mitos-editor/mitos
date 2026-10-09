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
    let expected = expected_args
        .map(|args| serde_json::to_string(args).map(|args| format!("\"args\":{args}")))
        .transpose()?
        .into_iter()
        .collect::<Vec<_>>();
    fixture_matching(response, "command", "", &expected, false)
}

fn fixture_matching(
    response: Value,
    event: &str,
    filter: &str,
    expected: &[String],
    once: bool,
) -> anyhow::Result<(TempDir, Config)> {
    fn wat_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
    }

    let response = serde_json::to_vec(&response)?;
    let mut needles = Vec::new();
    let mut checks = String::from("i32.const 1\n");
    for expected in expected {
        let pointer = 8192 + needles.len();
        checks.push_str(&format!(
            "local.get $input local.get $length i32.const {pointer} i32.const {} call $contains i32.and\n",
            expected.len()
        ));
        needles.extend_from_slice(expected.as_bytes());
    }
    let wrong_args = br#"{"error":"unexpected event metadata"}"#;
    let duplicate = br#"{"error":"duplicate event"}"#;
    let command_event = format!("\"event\":\"{event}\"");
    let matched_response = if once {
        format!("global.get $matched i32.const 1 i32.add global.set $matched global.get $matched i32.const 1 i32.eq if (result i64) i64.const {} else i64.const {} end",
            (4096_u64 << 32) | response.len() as u64,
            (20000_u64 << 32) | duplicate.len() as u64)
    } else {
        format!("i64.const {}", (4096_u64 << 32) | response.len() as u64)
    };
    let wasm = wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (global $initialized (mut i32) (i32.const 0))
            (global $matched (mut i32) (i32.const 0))
            (data (i32.const 0) "{{}}")
            (data (i32.const 4096) "{}")
            (data (i32.const 8192) "{}")
            (data (i32.const 12288) "{}")
            (data (i32.const 16384) "{}")
            (data (i32.const 20000) "{}")
            (data (i32.const 24576) "{}")
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
                    local.get $input local.get $length
                    i32.const 24576 i32.const {filter_len} call $contains
                    i32.and
                    if (result i64)
                        {checks}
                        if (result i64)
                            {matched_response}
                        else
                            i64.const {wrong_args_buffer}
                        end
                    else
                        i64.const 2
                    end
                end))"#,
        wat_bytes(&response),
        wat_bytes(&needles),
        wat_bytes(wrong_args),
        wat_bytes(command_event.as_bytes()),
        wat_bytes(duplicate),
        wat_bytes(filter.as_bytes()),
        command_event_len = command_event.len(),
        filter_len = filter.len(),
        wrong_args_buffer = (12288_u64 << 32) | wrong_args.len() as u64,
    ))?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("fixture.wasm"), wasm)?;
    std::fs::write(
        dir.path().join("plugin.toml"),
        format!("abi-version = {}\nmodule = 'fixture.wasm'\nevents = ['{event}']\n[commands.run]\ndoc = 'Run the fixture guest'\n", plugin_sdk::ABI_VERSION),
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

fn metadata_fragments(metadata: Value) -> Vec<String> {
    metadata
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| format!("\"{key}\":{value}"))
        .collect()
}

fn assert_observed(app: &term::application::Application) {
    let (message, severity) = app.editor.get_status().unwrap();
    assert_eq!(message.as_ref(), "Event matched");
    assert_eq!(*severity, Severity::Info);
}

#[tokio::test(flavor = "multi_thread")]
async fn post_command_reports_canonical_arguments_origins_and_one_completion() -> anyhow::Result<()>
{
    let cases = [
        (
            ":fixture.run 'hello world' \"line%{cursor_line}\"<ret>",
            "fixture.run",
            json!({"args": ["hello world", "line1"], "origin": "prompt", "outcome": "success"}),
        ),
        (
            "\"a3<F12>",
            "fixture.run",
            json!({"args": [], "count": 3, "register": "a", "origin": "keymap", "outcome": "success"}),
        ),
        (
            ":hello 'hello world'<ret>",
            "fixture.run",
            json!({"args": ["hello world"], "origin": "custom", "custom-command": "hello", "outcome": "success"}),
        ),
        (
            "<space>?fixture.run<ret>",
            "fixture.run",
            json!({"args": [], "origin": "palette", "outcome": "success"}),
        ),
        (
            ":g 1<ret>",
            "goto",
            json!({"args": ["1"], "origin": "prompt", "outcome": "success"}),
        ),
        (
            ":1<ret>",
            "goto",
            json!({"args": ["1"], "origin": "prompt", "outcome": "success"}),
        ),
        (
            ":echo<ret>",
            "echo",
            json!({"args": [], "origin": "prompt", "outcome": "error"}),
        ),
        (
            ":buffer-close-others<ret>",
            "buffer-close-others",
            json!({"flags": {}, "origin": "prompt", "outcome": "success"}),
        ),
        (
            ":buffer-close-others --skip-visible<ret>",
            "buffer-close-others",
            json!({"flags": {"skip-visible": ""}, "origin": "prompt", "outcome": "success"}),
        ),
        (
            ":missing.run<ret>",
            "missing.run",
            json!({"origin": "prompt", "outcome": "error", "error": "no such command: 'missing.run'"}),
        ),
    ];
    for (keys, command, metadata) in cases {
        let (_dir, mut config) = fixture_matching(
            json!({"actions": [{"type": "status", "message": "Event matched"}]}),
            "post-command",
            &format!("\"command\":\"{command}\""),
            &metadata_fragments(metadata),
            true,
        )?;
        config.keys = toml::from_str("[normal]\nF12 = ':fixture.run'\n")?;
        config.editor.commands = CustomCommands::new(vec![CustomCommand {
            commands: vec![":fixture.run %arg{0}".into()],
            ..CustomCommand::default()
        }
        .named(":hello".into())]);
        let mut app = AppBuilder::new().with_config(config).build()?;
        test_key_sequences(&mut app, vec![(Some(keys), Some(&assert_observed))], false).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn post_command_waits_for_callbacks_and_follow_up_keys() -> anyhow::Result<()> {
    for (keys, command, origin, extra) in [
        (
            ":hello<ret>",
            "@iX<esc>",
            "custom",
            vec!["\"text\":\"Xabc\\n\"".to_owned()],
        ),
        (
            ":partial<ret>",
            "@iX<esc>",
            "custom",
            vec![
                "\"text\":\"Xabc\\n\"".to_owned(),
                "\"outcome\":\"success\"".to_owned(),
            ],
        ),
        (
            ":find<ret>b",
            "find_next_char",
            "custom",
            vec!["\"anchor\":0,\"head\":2".to_owned()],
        ),
        (
            "f<esc>",
            "find_next_char",
            "keymap",
            vec!["\"outcome\":\"cancelled\"".to_owned()],
        ),
        (
            "<space>?command_mode<ret>",
            "command_mode",
            "palette",
            Vec::new(),
        ),
    ] {
        let mut expected = metadata_fragments(json!({"command": command, "origin": origin}));
        expected.extend(extra);
        let (_dir, mut config) = fixture_matching(
            json!({"actions": [{"type": "status", "message": "Event matched"}]}),
            "post-command",
            &if origin == "palette" {
                "\"origin\":\"palette\"".to_owned()
            } else {
                format!("\"command\":\"{command}\"")
            },
            &expected,
            true,
        )?;
        config.editor.commands = CustomCommands::new(vec![
            CustomCommand {
                commands: vec!["@iX<esc>".into()],
                ..CustomCommand::default()
            }
            .named(":hello".into()),
            CustomCommand {
                commands: vec!["@iX<esc>".into(), ":missing.run".into()],
                ..CustomCommand::default()
            }
            .named(":partial".into()),
            CustomCommand {
                commands: vec!["find_next_char".into()],
                ..CustomCommand::default()
            }
            .named(":find".into()),
        ]);
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text("#[a|]#bc\n")
            .build()?;
        let after = |app: &term::application::Application| {
            assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "after");
        };
        let mut inputs: Vec<(
            Option<&str>,
            Option<&dyn Fn(&term::application::Application)>,
        )> = vec![(Some(keys), Some(&assert_observed))];
        if origin == "palette" {
            inputs.push((Some("echo after<ret>"), Some(&after)));
        }
        test_key_sequences(&mut app, inputs, false).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insertion_and_terminal_focus_hooks_capture_the_originating_view() -> anyhow::Result<()> {
    let mut expected = metadata_fragments(json!({"character": "μ", "source": "insert-char"}));
    expected.push("\"text\":\"μ\\n\"".to_owned());
    expected.push("\"view\":{\"id\":".to_owned());
    let (_dir, config) = fixture_matching(
        json!({"actions": [{"type": "status", "message": "Event matched"}]}),
        "post-insert-char",
        "",
        &expected,
        true,
    )?;
    let mut app = AppBuilder::new().with_config(config).build()?;
    test_key_sequences(&mut app, vec![(Some("iμ"), Some(&assert_observed))], false).await?;

    #[cfg(windows)]
    use crossterm::event::Event;
    #[cfg(not(windows))]
    use termina::event::Event;
    #[cfg(not(windows))]
    let (lost, gained) = (Event::FocusOut, Event::FocusIn);
    #[cfg(windows)]
    let (lost, gained) = (Event::FocusLost, Event::FocusGained);
    for (event, focused) in [
        ("terminal-focus-lost", false),
        ("terminal-focus-gained", true),
    ] {
        let (_dir, config) = fixture_matching(
            json!({"actions": [{"type": "status", "message": "Event matched"}]}),
            event,
            "",
            &metadata_fragments(json!({"focused": focused})),
            true,
        )?;
        let mut app = AppBuilder::new().with_config(config).build()?;
        // Focus events must pass through modal prompts to the editor view.
        for key in ui_core::input::parse_macro(":")? {
            #[cfg(not(windows))]
            let key = termina::event::KeyEvent::from(key);
            #[cfg(windows)]
            let key = crossterm::event::KeyEvent::from(key);
            app.handle_terminal_events(Ok(Event::Key(key))).await;
        }
        app.handle_terminal_events(Ok(lost.clone())).await;
        app.handle_terminal_events(Ok(lost.clone())).await;
        if focused {
            app.handle_terminal_events(Ok(gained.clone())).await;
            app.handle_terminal_events(Ok(gained.clone())).await;
        }
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        assert!(app.event_loop_until_idle(&mut rx).await);
        assert_observed(&app);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn mode_hooks_follow_prompt_custom_keymap_and_palette_dispatch() -> anyhow::Result<()> {
    for (keys, command, origin, old, new) in [
        ("i", "insert_mode", "keymap", "normal", "insert"),
        (":hello<ret>", "insert_mode", "custom", "normal", "insert"),
        (
            "<space>?insert_mode<ret>",
            "insert_mode",
            "palette",
            "normal",
            "insert",
        ),
        ("v:new<ret>", "new", "prompt", "select", "normal"),
    ] {
        let (_dir, mut config) = fixture_matching(
            json!({"actions": [{"type": "status", "message": "Event matched"}]}),
            "mode-changed",
            &format!("\"command\":\"{command}\""),
            &metadata_fragments(
                json!({"old-mode": old, "new-mode": new, "origin": origin, "mode": new}),
            ),
            true,
        )?;
        config.editor.commands = CustomCommands::new(vec![CustomCommand {
            commands: vec!["insert_mode".into()],
            ..CustomCommand::default()
        }
        .named(":hello".into())]);
        let mut app = AppBuilder::new().with_config(config).build()?;
        test_key_sequences(&mut app, vec![(Some(keys), Some(&assert_observed))], false).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn non_key_input_completes_pending_commands_as_cancelled() -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{Event, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
    #[cfg(not(windows))]
    use termina::event::{Event, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

    for paste in [false, true] {
        let (_dir, config) = fixture_matching(
            json!({"actions": [{"type": "status", "message": "Event matched"}]}),
            "post-command",
            "\"command\":\"find_next_char\"",
            &metadata_fragments(json!({"origin": "keymap", "outcome": "cancelled"})),
            true,
        )?;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text("#[a|]#bc\n")
            .build()?;
        for key in ui_core::input::parse_macro("f")? {
            app.handle_terminal_events(Ok(Event::Key(KeyEvent::from(key))))
                .await;
        }
        let input = if paste {
            Event::Paste("X".into())
        } else {
            let (view, doc) = view::current_ref!(app.editor);
            let inner = view.inner_area(doc);
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                row: inner.y,
                column: inner.x,
                modifiers: ui_core::input::KeyModifiers::empty().into(),
            })
        };
        app.handle_terminal_events(Ok(input)).await;
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        assert!(app.event_loop_until_idle(&mut rx).await);
        assert_observed(&app);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_completion_waits_for_its_actual_save_result() -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{Event, KeyEvent};
    #[cfg(not(windows))]
    use termina::event::{Event, KeyEvent};

    for fail in [false, true] {
        let (_guest, mut config) = fixture_matching(
            json!({"actions": [{"type": "status", "message": "Event matched"}]}),
            "post-command",
            "\"command\":\"write\"",
            &metadata_fragments(json!({
                "origin": "prompt", "args": [], "flags": {"no-format": ""},
                "outcome": if fail { "error" } else { "success" },
            })),
            true,
        )?;
        config.editor.auto_format = false;
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("saved.txt");
        std::fs::write(&source, "original\n")?;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_file(&source, None)
            .build()?;

        // Input dispatch submits a future; no write has been polled yet.
        for key in ui_core::input::parse_macro("iX<esc>:write --no-format<ret>")? {
            app.handle_terminal_events(Ok(Event::Key(KeyEvent::from(key))))
                .await;
        }
        assert_eq!(app.editor.write_count, 1);
        app.editor.set_status("write still pending");
        app.editor.poll_plugin_events();
        assert_eq!(app.editor.get_status().unwrap().0, "write still pending");

        if fail {
            // The path was valid when accepted. This fails inside the owned
            // write future, rather than during argument parsing/preparation.
            std::fs::remove_dir_all(directory.path())?;
        } else {
            // A later, unrelated error must not become this write's outcome.
            app.editor.set_error(|| "unrelated later status");
        }
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        assert!(app.event_loop_until_idle(&mut rx).await);
        assert_observed(&app);
        if !fail {
            assert_eq!(std::fs::read_to_string(&source)?, "Xoriginal\n");
        }
        assert!(app.close().await.is_empty());
        assert_observed(&app);
    }
    Ok(())
}
