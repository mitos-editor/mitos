//! Native plugin dialogs and command composition with real guest callbacks.

use super::helpers::{run_event_loop_until_idle, test_config, test_key_sequences, AppBuilder};
use plugin_api::{
    ui::{
        BuiltinCommand, BuiltinInvocation, KeymapMode, PluginKeybinding, UiKind, UiOrigin, UiRow,
    },
    Action, Response,
};

#[path = "../../../view/tests/support/plugin_guest.rs"]
mod guest;

fn effects(actions: Vec<Action>) -> Response {
    Response {
        actions,
        error: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prompts_and_pickers_deliver_targeted_native_results() -> anyhow::Result<()> {
    for (kind, input, expected) in [
        (
            UiKind::Prompt {
                title: "Choose a value".into(),
                initial: "prefix ".into(),
            },
            "μ<ret>",
            vec![r#""kind":"prompt""#.into(), r#""text":"prefix μ""#.into()],
        ),
        (
            UiKind::Picker {
                title: "Choose a row".into(),
                rows: vec![
                    UiRow {
                        id: "alpha".into(),
                        label: "Alpha".into(),
                        description: "first".into(),
                        preview: Some("cached first\npreview".into()),
                        location: None,
                    },
                    UiRow {
                        id: "beta".into(),
                        label: "Beta".into(),
                        description: "second".into(),
                        preview: Some("cached second\npreview".into()),
                        location: None,
                    },
                ],
            },
            "<down><ret>",
            vec![r#""row":"beta""#.into(), r#""action":"replace""#.into()],
        ),
    ] {
        let dir = tempfile::tempdir()?;
        let mut config = test_config();
        config.plugins.insert(
            "fixture".into(),
            guest::observing(
                dir.path(),
                &[],
                &[
                    guest::Route {
                        event: "command",
                        response: effects(vec![Action::ShowUi {
                            request: 1,
                            origin: None,
                            kind,
                        }]),
                        ..guest::Route::default()
                    },
                    guest::Route {
                        event: "ui-result",
                        response: guest::status("native result observed"),
                        expected,
                        ..guest::Route::default()
                    },
                ],
                Some("ui-result"),
            )?,
        );
        let mut app = AppBuilder::new().with_config(config).build()?;
        let observed = |app: &term::application::Application| {
            assert_eq!(app.editor.get_status().unwrap().0, "native result observed");
            assert_eq!(view::doc!(app.editor).text().to_string(), "\n");
        };
        test_key_sequences(
            &mut app,
            vec![
                (Some(":fixture.run<ret>"), None),
                (Some(input), Some(&observed)),
            ],
            false,
        )
        .await?;
        assert_eq!(app.editor.error_revision(), 0);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn next_key_times_out_then_reused_request_id_opens_a_fresh_prompt() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut config = test_config();
    config.plugins.insert(
        "fixture".into(),
        guest::observing(
            dir.path(),
            &[],
            &[
                guest::Route {
                    event: "command",
                    response: effects(vec![Action::ShowUi {
                        request: 7,
                        origin: None,
                        kind: UiKind::NextKey {
                            title: "Press one key".into(),
                            timeout_ms: 5,
                        },
                    }]),
                    ..guest::Route::default()
                },
                guest::Route {
                    event: "ui-result",
                    filter: Some(r#""reason":"timed-out""#.into()),
                    response: effects(vec![Action::ShowUi {
                        request: 7,
                        origin: None,
                        kind: UiKind::Prompt {
                            title: "Fresh instance".into(),
                            initial: String::new(),
                        },
                    }]),
                    ..guest::Route::default()
                },
                guest::Route {
                    event: "ui-result",
                    filter: Some(r#""outcome":"accepted""#.into()),
                    response: guest::status("fresh dialog observed"),
                    expected: vec![r#""kind":"prompt""#.into(), r#""text":"μ""#.into()],
                    ..guest::Route::default()
                },
            ],
            Some("ui-result"),
        )?,
    );
    let mut app = AppBuilder::new().with_config(config).build()?;
    let observed = |app: &term::application::Application| {
        assert_eq!(app.editor.get_status().unwrap().0, "fresh dialog observed");
        assert_eq!(view::doc!(app.editor).text().to_string(), "\n");
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some(":fixture.run<ret>"), None),
            (Some("μ<ret>"), Some(&observed)),
        ],
        false,
    )
    .await?;
    assert_eq!(app.editor.error_revision(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reviewed_builtin_group_uses_one_original_preflight() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[a|]#bc\n").build()?;
    run_event_loop_until_idle(&mut app).await;
    let (view, doc) = view::current_ref!(app.editor);
    let origin = UiOrigin {
        view: view.id.as_u64(),
        document: doc.id().as_u64(),
        binding_revision: view.binding_revision(),
        version: doc.version(),
        selection_revision: doc.selection_revision(view.id).unwrap(),
    };
    let dir = tempfile::tempdir()?;
    let plugin = guest::observing(
        dir.path(),
        &[],
        &[
            guest::Route {
                event: "command",
                response: effects(vec![Action::InvokeBuiltin {
                    request: 1,
                    origin,
                    commands: vec![
                        BuiltinInvocation {
                            command: BuiltinCommand::SelectAll,
                            count: None,
                        },
                        BuiltinInvocation {
                            command: BuiltinCommand::DeleteSelectionNoYank,
                            count: None,
                        },
                    ],
                }]),
                ..guest::Route::default()
            },
            guest::Route {
                event: "builtin-result",
                response: guest::status("composition observed"),
                expected: vec![r#""completed":2"#.into(), r#""error":null"#.into()],
                ..guest::Route::default()
            },
        ],
        Some("builtin-result"),
    )?;
    assert!(app
        .editor
        .reload_plugins(&[("fixture".into(), plugin)].into(), dir.path()));
    let observed = |app: &term::application::Application| {
        assert_eq!(app.editor.get_status().unwrap().0, "composition observed");
        assert_eq!(view::doc!(app.editor).text().to_string(), "");
    };
    test_key_sequences(
        &mut app,
        vec![(Some(":fixture.run<ret>"), Some(&observed))],
        false,
    )
    .await?;
    assert_eq!(app.editor.error_revision(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_keymap_registration_routes_to_its_declared_command() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut config = test_config();
    config.plugins.insert(
        "fixture".into(),
        guest::observing(
            dir.path(),
            &[],
            &[
                guest::Route {
                    event: "command",
                    response: effects(vec![Action::UpdateKeymap {
                        request: 1,
                        bindings: vec![PluginKeybinding {
                            mode: KeymapMode::Normal,
                            keys: vec!["F12".into()],
                            command: "run".into(),
                        }],
                    }]),
                    ..guest::Route::default()
                },
                guest::Route {
                    event: "keymap-result",
                    response: guest::status("keymap observed"),
                    expected: vec![r#""error":null"#.into()],
                    ..guest::Route::default()
                },
            ],
            Some("keymap-result"),
        )?,
    );
    let mut app = AppBuilder::new().with_config(config).build()?;
    let observed = |app: &term::application::Application| {
        assert_eq!(app.editor.get_status().unwrap().0, "keymap observed")
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some(":fixture.run<ret>"), Some(&observed)),
            (Some("<F12>"), Some(&observed)),
        ],
        false,
    )
    .await?;
    assert_eq!(app.editor.error_revision(), 0);
    Ok(())
}
