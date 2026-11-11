use super::*;
use loader::snippets::Catalog;

fn definitions() -> Catalog {
    Catalog::parse("[[snippet]]\nname='Greeting'\nprefix='hello'\nbody='${1|one,two|} $0'").unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn named_snippets_and_choice_commands_work_through_terminal_keymaps() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    app.editor.set_snippet_catalog(definitions());
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":snippet-insert Greeting<ret>"),
                Some(&|app| {
                    assert!(view::doc!(app.editor)
                        .text()
                        .to_string()
                        .starts_with("one "));
                    assert_eq!(app.editor.mode(), view::document::Mode::Insert);
                }),
            ),
            (
                Some("<C-g>n"),
                Some(&|app| {
                    assert!(view::doc!(app.editor)
                        .text()
                        .to_string()
                        .starts_with("two "))
                }),
            ),
            (
                Some("<C-g>p"),
                Some(&|app| {
                    assert!(view::doc!(app.editor)
                        .text()
                        .to_string()
                        .starts_with("one "))
                }),
            ),
            (
                Some("X<esc>u"),
                Some(&|app| assert!(!view::doc!(app.editor).text().to_string().contains('X'))),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn snippet_picker_inserts_its_selected_definition() -> anyhow::Result<()> {
    let source = "[keys.normal]\n'+' = 'snippet_picker'";
    let source = source.to_owned();
    let mut config = test_config();
    config.keys = Config::load(Ok(&source), Err(term::config::ConfigLoadError::default()))
        .unwrap()
        .keys;
    let mut app = AppBuilder::new().with_config(config).build()?;
    app.editor.set_snippet_catalog(definitions());
    test_key_sequences(
        &mut app,
        vec![(
            Some("+Greeting<ret>"),
            Some(&|app| {
                assert!(view::doc!(app.editor)
                    .text()
                    .to_string()
                    .starts_with("one "));
                assert_eq!(app.editor.mode(), view::document::Mode::Insert);
            }),
        )],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn snippet_open_accepts_collection_paths_and_keeps_the_primary_file_default(
) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = AppBuilder::new().build()?;
    app.editor
        .set_snippet_resources(loader::snippets::Resources {
            user_file: directory.path().join("snippets.toml"),
        });
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":snippet-open team/rust.toml<ret>"),
                Some(&|app| {
                    assert!(view::doc!(app.editor)
                        .path()
                        .unwrap()
                        .ends_with(std::path::Path::new("snippets/team/rust.toml")));
                }),
            ),
            (
                Some(":snippet-open<ret>"),
                Some(&|app| {
                    assert!(view::doc!(app.editor)
                        .path()
                        .unwrap()
                        .ends_with("snippets.toml"));
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_completion_accepts_a_prefix_ending_in_punctuation() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.completion.auto_trigger = false;
    let mut app = AppBuilder::new().with_config(config).build()?;
    app.editor.set_snippet_catalog(Catalog::parse(
        "[[snippet]]\nname='For'\nprefix='for!'\nbody='${1:expanded}$0'",
    )?);
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("ifor!<C-x>"),
                Some(&|app| assert!(view::features::completion::is_shown(&app.editor))),
            ),
            (
                Some("<tab>"),
                Some(&|app| {
                    assert_eq!(
                        view::doc!(app.editor).text().to_string().trim_end(),
                        "expanded"
                    );
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn snippet_tabs_navigate_whitespace_with_smart_tab_disabled() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    assert!(
        !app.editor
            .config()
            .indent
            .smart_tab
            .as_ref()
            .unwrap()
            .enable
    );
    app.editor.set_snippet_catalog(Catalog::parse(
        "[[snippet]]\nname='Blank'\nbody='''$1\n$2\n$0''' ",
    )?);
    helpers::keys(&mut app, ":snippet-insert Blank<ret>").await?;
    let cursor = |app: &term::application::TestApplication| {
        let (view, doc) = view::current_ref!(app.editor);
        doc.selection(view.id).primary().head
    };
    let first = cursor(&app);
    let text = view::doc!(app.editor).text().to_string();
    helpers::keys(&mut app, "<tab>").await?;
    assert!(cursor(&app) > first);
    helpers::keys(&mut app, "<S-tab>").await?;
    assert_eq!(cursor(&app), first);
    assert_eq!(view::doc!(app.editor).text().to_string(), text);
    assert!(app.close().await.is_empty());
    Ok(())
}
