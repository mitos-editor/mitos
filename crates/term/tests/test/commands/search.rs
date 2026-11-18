use editor_core::search::SearchSyntax;
use stdx::rope::RopeSliceExt as _;
use term::application::TestApplication;
use tui::backend::Backend;
use view::{current_ref, register::SearchHistoryEntry};

use super::super::helpers::{keys, test, test_config, AppBuilder};

fn assert_prompt(app: &TestApplication, text: &str, syntax: &str) {
    let screen: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        screen.contains(&format!("search [{syntax}]:{text}")),
        "{screen}"
    );
    assert_eq!(
        app.terminal_backend()
            .clone()
            .get_cursor_position()
            .unwrap(),
        (
            (format!("search [{syntax}]:").len() + text.len()) as u16,
            app.terminal_backend().buffer().area.bottom() - 1
        )
            .into(),
    );
}

fn match_count(app: &TestApplication) -> usize {
    let (view, doc) = current_ref!(app.editor);
    doc.search_highlight(view.id).map_or(0, |query| {
        query
            .regex()
            .find_iter(doc.text().slice(..).regex_input())
            .count()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn literal_search_repeats_forward_backward_and_in_select_mode() -> anyhow::Result<()> {
    for (before, input, after) in [
        ("#[|x]# a.b axb a.b\n", "/a.b<ret>n", "x a.b #[|axb]# a.b\n"),
        (
            "#[|x]# a.b axb a.b\n",
            "/<A-r>a.b<ret>n",
            "x a.b axb #[|a.b]#\n",
        ),
        (
            "a.b axb a.b #[|x]#\n",
            "?<A-r>a.b<ret>N",
            "#[|a.b]# axb a.b x\n",
        ),
        (
            "#[|x]# a.b axb a.b\n",
            "v/<A-r>a.b<ret>",
            "#(|x)# #[|a.b]# axb a.b\n",
        ),
        (
            "#[|x]# Foo.bar foo.bar Foo.bar\n",
            "/<A-r>Foo.bar<ret>n",
            "x Foo.bar foo.bar #[|Foo.bar]#\n",
        ),
        (
            "#[|x]# foo.bar Foo.bar\n",
            "/<A-r>foo.bar<ret>n",
            "x foo.bar #[|Foo.bar]#\n",
        ),
        (
            "#[|x]# %foo(bar[\\ \n",
            "/<A-r>%foo(bar[\\<ret>",
            "x #[|%foo(bar[\\]# \n",
        ),
    ] {
        test((before, input, after)).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn literal_search_toggle_updates_preview_without_editing_input() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[|x]# a.b axb a.b\n")
        .build()?;
    keys(&mut app, "/a.b").await?;
    assert_prompt(&app, "a.b", "regex");
    assert_eq!(match_count(&app), 3);
    keys(&mut app, "<A-r>").await?;
    assert_prompt(&app, "a.b", "literal");
    assert_eq!(match_count(&app), 2);
    keys(&mut app, "<A-r>").await?;
    assert_prompt(&app, "a.b", "regex");
    assert_eq!(match_count(&app), 3);
    keys(&mut app, "<esc>/<A-r>[").await?;
    assert_prompt(&app, "[", "literal");
    keys(&mut app, "<A-r>").await?;
    assert_eq!(match_count(&app), 0);
    keys(&mut app, "<A-r><ret>").await?;
    assert_eq!(app.editor.first_register_value('/').as_deref(), Some(r"\["));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn literal_search_history_restores_modes_and_the_unsubmitted_draft() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[|x]# a.b axb a.b\n")
        .build()?;
    keys(&mut app, "/a.b<ret>/<A-r>a.b<ret>/").await?;
    assert_prompt(&app, "", "regex");
    assert_eq!(
        app.editor.registers.search_history('/').collect::<Vec<_>>(),
        [
            SearchHistoryEntry {
                text: "a.b".into(),
                syntax: SearchSyntax::Literal
            },
            SearchHistoryEntry::regex("a.b".into()),
        ]
    );
    keys(&mut app, "<up>").await?;
    assert_prompt(&app, "a.b", "literal");
    assert_eq!(match_count(&app), 2);
    keys(&mut app, "<up>").await?;
    assert_prompt(&app, "a.b", "regex");
    assert_eq!(match_count(&app), 3);
    keys(&mut app, "<down>").await?;
    assert_prompt(&app, "a.b", "literal");
    keys(&mut app, "<down>").await?;
    assert_prompt(&app, "", "regex");
    keys(&mut app, "a<A-r><up>").await?;
    assert_prompt(&app, "a.b", "literal");
    keys(&mut app, "<down>").await?;
    assert_prompt(&app, "a", "literal");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn literal_search_empty_accept_right_arrow_and_completion_keep_literal_input(
) -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[|x]# a.b axb a.b\n")
        .build()?;
    keys(&mut app, "/<A-r>a.b<ret>/<ret>").await?;
    assert_eq!(match_count(&app), 2);
    assert_eq!(
        app.editor.first_register_value('/').as_deref(),
        Some(r"a\.b")
    );
    assert_eq!(
        app.editor
            .registers
            .search_history('/')
            .collect::<Vec<_>>()
            .len(),
        1
    );
    keys(&mut app, "/<right>").await?;
    assert_prompt(&app, "a.b", "literal");
    keys(&mut app, "<esc>/<A-r>a<tab>").await?;
    assert_prompt(&app, "a.b", "literal");
    keys(&mut app, "<ret>").await?;
    assert_eq!(
        app.editor.first_register_value('/').as_deref(),
        Some(r"a\.b")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn literal_search_cancel_restores_highlights_selection_and_viewport() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[|x]# a.b axb a.b\n")
        .build()?;
    keys(&mut app, "/a.b<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    let selection = doc.selection(view.id).clone();
    let offset = doc.view_offset(view.id);
    let history = app.editor.registers.search_history('/').collect::<Vec<_>>();
    keys(&mut app, "/<A-r>a.b<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.selection(view.id), &selection);
    assert_eq!(doc.view_offset(view.id), offset);
    assert_eq!(match_count(&app), 3);
    assert_eq!(
        app.editor.registers.search_history('/').collect::<Vec<_>>(),
        history
    );
    keys(&mut app, "/[<ret>").await?;
    assert_eq!(
        app.editor.registers.search_history('/').collect::<Vec<_>>(),
        history
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn literal_search_toggle_can_be_remapped_and_uses_named_registers() -> anyhow::Result<()> {
    let source = "[keys.prompt]\nC-t = 'toggle_search_syntax'\nA-r = 'no_op'".to_owned();
    let mut config = test_config();
    config.component_keys = term::config::Config::load(Ok(&source), Err(Default::default()))
        .unwrap()
        .component_keys;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[|x]# a.b axb a.b\n")
        .build()?;
    keys(&mut app, "\"a/a.b<A-r>").await?;
    assert_prompt(&app, "a.b", "regex");
    keys(&mut app, "<C-t><ret>n").await?;
    assert_eq!(app.editor.registers.last_search_register, 'a');
    assert_eq!(
        app.editor.first_register_value('a').as_deref(),
        Some(r"a\.b")
    );
    assert_eq!(match_count(&app), 2);
    keys(&mut app, "\"a/<up>").await?;
    assert_prompt(&app, "a.b", "literal");
    Ok(())
}
