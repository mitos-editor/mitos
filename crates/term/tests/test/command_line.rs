use super::*;
use stdx::rope::RopeExt as _;

use ::diagnostics::Severity;
use editor_config::{CustomCommand, CustomCommands};

#[tokio::test(flavor = "multi_thread")]
async fn labeled_keybindings_execute_typable_commands_macros_and_sequences() -> anyhow::Result<()> {
    let source = r#"
        [keys.normal."+"]
        label = "Tools"
        e = { label = "Show message", command = ":echo typable" }
        m = { label = "Insert Unicode", command = "@i界<esc>" }
        s = { label = "Replace selection", command = ["select_all", ":echo sequence", "delete_selection"] }
    "#.to_owned();
    let mut config = test_config();
    config.keys = Config::load(Ok(&source), Err(term::config::ConfigLoadError::default()))
        .unwrap()
        .keys;
    let mut app = AppBuilder::new().with_config(config).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("+e"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "typable");
                }),
            ),
            (
                Some("+m"),
                Some(&|app| {
                    let (_, doc) = view::current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        format!("界{}", editor_core::NATIVE_LINE_ENDING.as_str())
                    );
                    assert_eq!(app.editor.mode(), view::document::Mode::Normal);
                }),
            ),
            (
                Some("+s"),
                Some(&|app| {
                    let (_, doc) = view::current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "");
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "sequence");
                }),
            ),
        ],
        false,
    )
    .await
}

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

    for name in [
        "terminal.true-color",
        "terminal.undercurl",
        "terminal.keyboard.kitty-protocol",
    ] {
        assert!(term::ui::completers::setting(&app.editor, name)
            .iter()
            .any(|(_, span)| span.content == name));
    }

    // An editor-only update must preserve frontend state and custom commands.
    let mut editor_config = (*app.editor.config()).clone();
    editor_config.scroll.padding = 11;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(editor_config)));

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":get true-color<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "true");
                    assert_eq!(app.editor.config().scroll.padding, 11);
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

#[tokio::test(flavor = "multi_thread")]
async fn feature_commands_use_canonical_paths_and_accept_legacy_aliases() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let names = term::ui::completers::setting(&app.editor, "lsp.inlay-hints");
    assert!(names
        .iter()
        .any(|(_, span)| span.content == "lsp.inlay-hints.enable"));
    assert!(!names
        .iter()
        .any(|(_, span)| span.content == "lsp.display-inlay-hints"));
    test_key_sequences(
        &mut app,
        vec![
            (Some(":set completion.trigger-delay 100<ret>"), None),
            (Some(":toggle lsp.display-inlay-hints<ret>"), None),
            (
                Some(":get lsp.inlay-hints.enable<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "true");
                    let config = app.editor.config();
                    assert!(config.lsp.inlay_hints.enable);
                    assert_eq!(config.completion.trigger_delay.as_millis(), 100);
                    assert!(config.lsp.messages.enable);
                }),
            ),
            (Some(":toggle lsp.inlay-hints.enable<ret>"), None),
            (
                Some(":get lsp.display-inlay-hints<ret>"),
                Some(&|app| assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "false")),
            ),
        ],
        false,
    )
    .await
}

fn goto_preview_app() -> anyhow::Result<term::application::Application> {
    let mut app = AppBuilder::new()
        .with_input_text("o#[|n]#e\ntwo\nthree\nfour\n")
        .build()?;
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(view.id, Selection::single(1, 2));
    doc.commit_changes();
    view.sync_changes(doc);
    Ok(app)
}

async fn goto_keys(app: &mut term::application::Application, input: &str) -> anyhow::Result<()> {
    for key in ui_core::input::parse_macro(input)? {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(key.into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(key.into());
        app.handle_terminal_events(Ok(event)).await;
    }
    Ok(())
}

fn assert_goto_cursor(app: &term::application::Application, position: usize) {
    let (view, doc) = view::current_ref!(app.editor);
    assert_eq!(
        doc.selection(view.id),
        &Selection::single(position, (position + 1).min(doc.text().len_chars())),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_preview_cancellation_restores_its_original_selection() -> anyhow::Result<()> {
    for keys in [
        ":goto 3<esc>",
        ":3<esc>",
        ":goto 3<backspace>4<esc>",
        ":goto 3x<esc>",
        ":3<C-u>echo changed<ret>",
    ] {
        let mut app = goto_preview_app()?;
        goto_keys(&mut app, keys).await?;
        assert_goto_cursor(&app, 1);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_confirmation_records_the_selection_before_previewing() -> anyhow::Result<()> {
    let mut app = goto_preview_app()?;
    goto_keys(&mut app, ":goto 3<ret>").await?;
    let (_, doc) = view::current_ref!(app.editor);
    assert_goto_cursor(&app, doc.text().line_to_char_idx(2));
    goto_keys(&mut app, "<C-o>").await?;
    assert_goto_cursor(&app, 1);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_keybinding_records_a_jump_without_a_prompt() -> anyhow::Result<()> {
    let mut config = test_config();
    let source = r#"[keys.normal]
        "+" = ":goto 3"
    "#
    .to_owned();
    config.keys = Config::load(Ok(&source), Err(term::config::ConfigLoadError::default()))
        .unwrap()
        .keys;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("o#[|n]#e\ntwo\nthree\nfour\n")
        .build()?;
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(view.id, Selection::single(1, 2));
    doc.commit_changes();
    view.sync_changes(doc);
    goto_keys(&mut app, "+<C-o>").await?;
    assert_goto_cursor(&app, 1);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_preview_cannot_follow_a_rebound_view() -> anyhow::Result<()> {
    for ending in [
        "4<esc>",
        "4<ret>",
        "4x<backspace><ret>",
        "4<C-u>goto 2<ret>",
        "<backspace>2<ret>",
    ] {
        let mut app = goto_preview_app()?;
        goto_keys(&mut app, ":goto 3").await?;
        app.editor.new_file(view::editor::Action::Replace);
        let (view, doc) = view::current!(app.editor);
        let transaction = editor_core::Transaction::change(
            doc.text(),
            std::iter::once((0, 0, Some("alpha\nbeta\ngamma\ndelta\n".into()))),
        );
        view::editing::apply(doc, &transaction, view.id).unwrap();
        doc.set_selection(view.id, Selection::single(1, 2));
        let jumps = view.jumps.iter().count();
        goto_keys(&mut app, ending).await?;
        assert_goto_cursor(&app, 1);
        assert_eq!(view::view!(app.editor).jumps.iter().count(), jumps);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_preview_cannot_resume_after_switching_away_and_back() -> anyhow::Result<()> {
    for ending in ["4<esc>", "4<ret>"] {
        let mut app = goto_preview_app()?;
        let original = view::view!(app.editor).document_id();
        goto_keys(&mut app, ":goto 3").await?;
        app.editor.new_file(view::editor::Action::Replace);
        app.editor.switch(original, view::editor::Action::Replace);
        let (view, doc) = view::current!(app.editor);
        doc.set_selection(view.id, Selection::single(2, 3));
        let jumps = view.jumps.iter().count();
        goto_keys(&mut app, ending).await?;
        assert_goto_cursor(&app, 2);
        assert_eq!(view::view!(app.editor).jumps.iter().count(), jumps);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_preview_cancellation_restores_only_its_original_split() -> anyhow::Result<()> {
    for ending in [
        "<esc>",
        "4<esc>",
        "4<ret>",
        "<backspace>2<ret>",
        "<C-u>goto 2<ret>",
    ] {
        let mut app = goto_preview_app()?;
        app.editor.switch(
            view::view!(app.editor).document_id(),
            view::editor::Action::HorizontalSplit,
        );
        let (view, doc) = view::current!(app.editor);
        doc.set_selection(view.id, Selection::single(1, 2));
        let original_view = view.id;
        let other_view = app
            .editor
            .tree
            .views()
            .find(|(view, _)| view.id != original_view)
            .unwrap()
            .0
            .id;
        goto_keys(&mut app, ":goto 3").await?;
        app.editor.focus(other_view);
        let (view, doc) = view::current!(app.editor);
        doc.set_selection(view.id, Selection::single(2, 3));
        goto_keys(&mut app, ending).await?;
        assert_goto_cursor(&app, 2);
        let doc = app
            .editor
            .document(view::view!(app.editor).document_id())
            .unwrap();
        assert_eq!(doc.selection(original_view), &Selection::single(1, 2));
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_preview_ignores_a_closed_origin_split() -> anyhow::Result<()> {
    let mut app = goto_preview_app()?;
    app.editor.switch(
        view::view!(app.editor).document_id(),
        view::editor::Action::HorizontalSplit,
    );
    goto_keys(&mut app, ":goto 3").await?;
    app.editor.close(app.editor.tree.focus());
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(view.id, Selection::single(2, 3));
    goto_keys(&mut app, "4<ret>").await?;
    assert_goto_cursor(&app, 2);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_preview_rejects_a_selection_snapshot_after_text_changes() -> anyhow::Result<()> {
    for ending in ["<esc>", "4<ret>"] {
        let mut app = goto_preview_app()?;
        goto_keys(&mut app, ":goto 3").await?;
        let (view, doc) = view::current!(app.editor);
        let transaction = editor_core::Transaction::change(
            doc.text(),
            std::iter::once((0, doc.text().len_chars(), None)),
        );
        view::editing::apply(doc, &transaction, view.id).unwrap();
        goto_keys(&mut app, ending).await?;
        assert_goto_cursor(&app, 0);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_preview_rolls_back_when_the_argument_becomes_invalid() -> anyhow::Result<()> {
    let mut app = goto_preview_app()?;
    goto_keys(&mut app, ":goto 3x").await?;
    assert_goto_cursor(&app, 1);
    goto_keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn custom_commands_resume_in_order_after_shell_expansion() -> anyhow::Result<()> {
    let config = config_with_custom_command(
        "capture",
        &[":set-register a %sh{printf first}", ":echo %reg{a}"],
    );
    test_key_sequence(
        &mut AppBuilder::new().with_config(config).build()?,
        Some(":capture<ret>"),
        Some(&|app| {
            assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "first");
        }),
        false,
    )
    .await
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn shell_expansions_allow_cancellation_and_new_submissions() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":echo %sh{sleep 30; echo late}<ret><C-c>"),
                Some(&|app| {
                    assert_eq!(
                        app.editor.get_status().unwrap().0.as_ref(),
                        "Command cancelled"
                    );
                }),
            ),
            (
                Some(":echo %sh{sleep 30; echo late}<ret>:echo current<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "current");
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_statusline_elements_render_cursor_encoding_and_register_count(
) -> anyhow::Result<()> {
    use editor_config::StatusLineElement as E;
    let mut config = test_config();
    config.editor.statusline.left = vec![E::Row, E::Col, E::FileEncoding, E::YankedSelections];
    config.editor.statusline.right.clear();
    config.editor.statusline.always_show_encoding = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("one\n  #[|t]#wo")
        .build()?;
    app.editor
        .registers
        .write('"', vec!["one".into(), "two".into()])?;
    keys(&mut app, "<esc>").await?;
    let backend = app.terminal_backend();
    let text: String = backend
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains(" 2  3  UTF-8  2 sels yanked "), "{text:?}");
    Ok(())
}
