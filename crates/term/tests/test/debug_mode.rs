use super::helpers::{test_key_sequences, AppBuilder};
use view::{current_ref, document::Mode};

#[tokio::test(flavor = "multi_thread")]
async fn configured_debug_mode_navigates_edits_and_renders_its_status() -> anyhow::Result<()> {
    let source = r#"
        [keys.normal]
        F12 = "debug_mode"
        [keys.debug]
        F11 = "move_char_right"
    "#
    .to_owned();
    let config = term::config::Config::load(Ok(&source), Ok(String::new())).unwrap();
    let config = term::config::Config {
        editor: editor_config::Config {
            statusline: config.editor.statusline.clone(),
            ..super::helpers::test_editor_config()
        },
        ..config
    };
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[|a]#bc\n")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<F12><F11>"),
                Some(&|app| {
                    assert_eq!(app.editor.mode(), Mode::Debug);
                    assert!(app.editor.autoinfo.is_none());
                    let (view, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.selection(view.id)
                            .primary()
                            .cursor(doc.text().slice(..)),
                        1
                    );
                    let backend = app.terminal_backend();
                    let screen: String = backend
                        .buffer()
                        .content()
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(screen.contains("DBG"));
                }),
            ),
            (
                Some("<C-w>v<C-w>w"),
                Some(&|app| assert_eq!(app.editor.mode(), Mode::Debug)),
            ),
            (
                Some("aX<esc>"),
                Some(&|app| {
                    assert_eq!(app.editor.mode(), Mode::Debug);
                    assert!(current_ref!(app.editor).1.text().to_string().contains('X'));
                }),
            ),
            (
                Some("<F12><esc>"),
                Some(&|app| assert_eq!(app.editor.mode(), Mode::Normal)),
            ),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_workflow_keeps_navigation_and_offers_searchable_binding_help() -> anyhow::Result<()>
{
    let source = r#"
        [keys.debug]
        F11 = "dap_next"
    "#
    .to_owned();
    let mut config = term::config::Config::load(Ok(&source), Ok(String::new())).unwrap();
    config.editor = super::helpers::test_editor_config();
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[|a]#bc\n")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>Glh"),
                Some(&|app| {
                    assert_eq!(app.editor.mode(), Mode::Debug);
                    assert!(app.editor.autoinfo.is_none());
                    assert!(app.editor.debugger().is_empty());
                    let (view, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.selection(view.id)
                            .primary()
                            .cursor(doc.text().slice(..)),
                        0
                    );
                }),
            ),
            (
                Some("?dap_next"),
                Some(&|app| {
                    let backend = app.terminal_backend();
                    let screen: String = backend
                        .buffer()
                        .content()
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(screen.contains("dap_next"), "{screen}");
                    assert!(screen.contains("<F11>"), "{screen}");
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| assert_eq!(app.editor.mode(), Mode::Debug)),
            ),
            (
                Some("?move_char_left"),
                Some(&|app| {
                    let backend = app.terminal_backend();
                    let screen: String = backend
                        .buffer()
                        .content()
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(screen.contains("Debug Commands"));
                    assert_eq!(screen.matches("move_char_left").count(), 1, "{screen}");
                }),
            ),
            (Some("<esc>"), None),
            (
                Some("?normal_mode<ret>"),
                Some(&|app| assert_eq!(app.editor.mode(), Mode::Normal)),
            ),
            (
                Some("v<space>G"),
                Some(&|app| assert_eq!(app.editor.mode(), Mode::Debug)),
            ),
            (
                Some("<esc>"),
                Some(&|app| assert_eq!(app.editor.mode(), Mode::Normal)),
            ),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_returns_to_debug_mode_and_repeat_uses_the_current_command_mode(
) -> anyhow::Result<()> {
    let source = r#"
        [keys.normal]
        F10 = "select_mode"
        [keys.insert]
        F9 = "normal_mode"
        F10 = "select_mode"
    "#
    .to_owned();
    let mut config = term::config::Config::load(Ok(&source), Ok(String::new())).unwrap();
    config.editor = super::helpers::test_editor_config();
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[|a]#bc\n")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    let debug = |app: &term::application::Application| assert_eq!(app.editor.mode(), Mode::Debug);
    let normal = |app: &term::application::Application| assert_eq!(app.editor.mode(), Mode::Normal);
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>GaX<esc>"),
                Some(&|app| {
                    debug(app);
                    assert_eq!(current_ref!(app.editor).1.text(), "aXbc\n");
                }),
            ),
            (
                Some("."),
                Some(&|app| {
                    debug(app);
                    assert_eq!(current_ref!(app.editor).1.text(), "aXXbc\n");
                }),
            ),
            (
                Some("u"),
                Some(&|app| {
                    debug(app);
                    assert_eq!(current_ref!(app.editor).1.text(), "aXbc\n");
                }),
            ),
            (
                Some("u<esc>gh."),
                Some(&|app| {
                    normal(app);
                    assert_eq!(current_ref!(app.editor).1.text(), "aXbc\n");
                }),
            ),
            (Some("<space>GaQ<F10><esc>"), Some(&debug)),
            (Some("<F10>v"), Some(&debug)),
            (Some("<F10>y"), Some(&debug)),
            (Some("<F10>d"), Some(&debug)),
            (Some("<F10><A-r>replacement<ret>"), Some(&debug)),
            (Some("aZ<F9>"), Some(&normal)),
            (Some("iR<esc><F10><esc>"), Some(&normal)),
            (Some("<space>GaS<esc><esc>"), Some(&normal)),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await?;
    Ok(())
}
