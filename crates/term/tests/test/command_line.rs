use super::*;

use editor_core::diagnostic::Severity;
use view::custom_commands::{CustomCommand, CustomCommands};

#[tokio::test(flavor = "multi_thread")]
async fn terminal_options_remain_available_through_commands() -> anyhow::Result<()> {
    let mut config = test_config();
    config.terminal.true_color = true;
    config.editor.commands = CustomCommands::new(vec![CustomCommand {
        commands: vec![":echo retained".into()],
        ..CustomCommand::default()
    }
    .named(":check-custom".into())]);
    let mut app = AppBuilder::new().with_config(config).build()?;

    for name in ["true-color", "undercurl", "kitty-keyboard-protocol"] {
        assert!(term::ui::completers::setting(&app.editor, name)
            .iter()
            .any(|(_, span)| span.content == name));
    }

    // An editor-only update must preserve frontend state and custom commands.
    let mut editor_config = (*app.editor.config()).clone();
    editor_config.scrolloff = 11;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(editor_config)));

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":get true-color<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "true");
                    assert_eq!(app.editor.config().scrolloff, 11);
                }),
            ),
            (Some(":set true-color false<ret>"), None),
            (
                Some(":get true-color<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "false");
                }),
            ),
            (Some(":toggle undercurl<ret>"), None),
            (
                Some(":get undercurl<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "true");
                }),
            ),
            (Some(":set kitty-keyboard-protocol disabled<ret>"), None),
            (
                Some(":get kitty-keyboard-protocol<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "\"disabled\"");
                }),
            ),
            (
                Some(":toggle kitty-keyboard-protocol disabled enabled<ret>"),
                None,
            ),
            (
                Some(":get kitty-keyboard-protocol<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "\"enabled\"");
                }),
            ),
            (Some(":set scrolloff 9<ret>"), None),
            (
                Some(":get scrolloff<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "9");
                }),
            ),
            (
                Some(":check-custom<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "retained");
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn history_completion() -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new().build()?,
        Some(":asdf<ret>:theme d<C-n><tab>"),
        Some(&|app| {
            assert!(!app.editor.is_err());
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_reset_anchor() -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new().build()?,
        Some(":string wider than the terminal window causing the anchor location to be non zero which would panic when the line is deleted<C-u>"),
        Some(&|app| {
            assert!(!app.editor.is_err());
        }),
        false,
    )
    .await?;

    Ok(())
}

async fn test_statusline(
    line: &str,
    expected_status: &str,
    expected_severity: Severity,
) -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new().build()?,
        Some(&format!("{line}<ret>")),
        Some(&|app| {
            let (status, &severity) = app.editor.get_status().unwrap();
            assert_eq!(
                severity, expected_severity,
                "'{line}' printed {severity:?}: {status}"
            );
            assert_eq!(status.as_ref(), expected_status);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn variable_expansion() -> anyhow::Result<()> {
    test_statusline(r#":echo %{cursor_line}"#, "1", Severity::Info).await?;
    // Double quotes can be used with expansions:
    test_statusline(
        r#":echo "line%{cursor_line}line""#,
        "line1line",
        Severity::Info,
    )
    .await?;
    // Within double quotes you can escape the percent token for an expansion by doubling it.
    test_statusline(
        r#":echo "%%{cursor_line}""#,
        "%{cursor_line}",
        Severity::Info,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_expansion() -> anyhow::Result<()> {
    test_statusline(r#":echo %u{20}"#, " ", Severity::Info).await?;
    test_statusline(r#":echo %u{0020}"#, " ", Severity::Info).await?;
    test_statusline(r#":echo %u{25CF}"#, "●", Severity::Info).await?;
    // Not a valid Unicode codepoint:
    test_statusline(
        r#":echo %u{deadbeef}"#,
        "'echo': could not interpret 'deadbeef' as a Unicode character code",
        Severity::Error,
    )
    .await?;

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn shell_expansion() -> anyhow::Result<()> {
    test_statusline(
        r#":echo %sh{echo "hello world"}"#,
        "hello world",
        Severity::Info,
    )
    .await?;

    // Shell expansion is recursive.
    test_statusline(":echo %sh{echo '%{cursor_line}'}", "1", Severity::Info).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn register_expansion() -> anyhow::Result<()> {
    test_statusline(
        r#":set-register a hello world<ret>:echo %reg{a}"#,
        "hello world",
        Severity::Info,
    )
    .await?;
    test_statusline(r#":echo %reg{a}"#, "", Severity::Info).await?;
    test_statusline(
        r#":echo %reg{abc}"#,
        "'echo': Invalid register `abc`: should only be a single character",
        Severity::Error,
    )
    .await?;

    // Register expansion evaluation is *not* recursive.
    test_statusline(
        r#":set-register a b<ret>:set-register b hello<ret>:echo %reg{%reg{a}}"#,
        "'echo': Invalid register `%reg{a}`: should only be a single character",
        Severity::Error,
    )
    .await?;
    test_statusline(
        r#":set-register a hello<ret>:set-register b %%reg{a}<ret>:echo %reg{b}"#,
        "%reg{a}",
        Severity::Info,
    )
    .await?;

    // However, you can copy the contents of one register into another with this expansion if you
    // want to.
    test_statusline(
        r#":set-register a hello<ret>:set-register b %reg{a}<ret>:echo %reg{b}"#,
        "hello",
        Severity::Info,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn percent_escaping() -> anyhow::Result<()> {
    test_statusline(
        r#":sh echo hello 10%"#,
        "'run-shell-command': '%' was not properly escaped. Please use '%%'",
        Severity::Error,
    )
    .await?;
    Ok(())
}

fn config_with_custom_command(name: &str, commands: &[&str]) -> Config {
    let mut config = test_config();
    config.editor.commands = CustomCommands::new(vec![CustomCommand {
        name: name.to_owned(),
        commands: commands
            .iter()
            .map(|command| (*command).to_owned())
            .collect(),
        ..CustomCommand::default()
    }]);
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_command_expands_positional_arguments() -> anyhow::Result<()> {
    let config = config_with_custom_command("say", &[":echo %arg{1} %arg{0}"]);
    test_key_sequence(
        &mut AppBuilder::new().with_config(config).build()?,
        Some(":say first second<ret>"),
        Some(&|app| {
            let (status, severity) = app.editor.get_status().unwrap();
            assert_eq!(status.as_ref(), "second first");
            assert_eq!(*severity, Severity::Info);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn escaped_custom_command_calls_the_builtin() -> anyhow::Result<()> {
    let config = config_with_custom_command("echo", &[":echo custom"]);
    let mut app = AppBuilder::new().with_config(config).build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":echo ignored<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "custom");
                }),
            ),
            (
                Some(":^echo builtin<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "builtin");
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_macro_runs_after_the_prompt_closes() -> anyhow::Result<()> {
    let config = config_with_custom_command("insert-greeting", &["@ihello<esc>"]);
    test_key_sequence(
        &mut AppBuilder::new().with_config(config).build()?,
        Some(":insert-greeting<ret>"),
        Some(&|app| {
            assert_eq!(
                view::doc!(app.editor).text().to_string(),
                format!("hello{}", editor_core::NATIVE_LINE_ENDING.as_str())
            );
        }),
        false,
    )
    .await
}
