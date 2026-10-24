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
                    assert_eq!(app.editor.mode(), Mode::Normal);
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
