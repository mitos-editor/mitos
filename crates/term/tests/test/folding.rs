use super::helpers::{keys, test_syntax_loader, AppBuilder};
use stdx::rope::RopeExt as _;
use view::{current_ref, editor::Action};

#[tokio::test(flavor = "multi_thread")]
async fn manual_folds_render_navigate_hit_test_and_reveal_search_results() -> anyhow::Result<()> {
    for newline in ["\n", "\r\n"] {
        let mut config = super::helpers::test_config();
        config.editor.line_number = editor_config::LineNumber::Relative;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text(format!(
                "#[header{newline}hidden{newline}end|]#{newline}next{newline}"
            ))
            .with_lang_loader(language::syntax::Loader::default())
            .build()?;
        keys(&mut app, "zfs").await?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(doc.folded_ranges(view.id).len(), 1);
        let area = view.inner_area(doc);
        let backend = app.terminal_backend();
        assert_eq!(backend.buffer()[(area.x + 6, area.y)].symbol(), "…");
        assert_eq!(backend.buffer()[(area.x, area.y + 1)].symbol(), "n");
        let number: String = (view.area.x..area.x)
            .map(|x| backend.buffer()[(x, area.y + 1)].symbol())
            .collect();
        assert_eq!(number.trim(), "1");
        assert_eq!(
            view.pos_at_screen_coords(doc, area.y, area.x + 10, true),
            Some(doc.folded_ranges(view.id)[0].range.start),
        );
        let next = doc.text().line_to_char_idx(3);
        assert_eq!(
            view.pos_at_screen_coords(doc, area.y + 1, area.x, true),
            Some(next)
        );
        keys(&mut app, "j").await?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            next
        );
        let backend = app.terminal_backend();
        let number: String = (view.area.x..area.x)
            .map(|x| backend.buffer()[(x, area.y)].symbol())
            .collect();
        assert_eq!(number.trim(), "1");
        keys(&mut app, "k/hidden<ret>").await?;
        let (view, doc) = current_ref!(app.editor);
        assert!(doc.folded_ranges(view.id).is_empty());
        assert_eq!(doc.cursor_line(view.id), 1);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_highlighting_continues_after_a_fold_larger_than_the_viewport() -> anyhow::Result<()>
{
    let mut app = AppBuilder::new()
        .with_input_text(format!(
            "#[f|]#n hidden() {{\n{}}}\nfn visible() {{}}\n",
            "    work();\n".repeat(200)
        ))
        .with_lang_loader(test_syntax_loader(None))
        .build()?;
    app.editor
        .set_theme(theme::Theme::from(toml::from_str::<toml::Value>(
            r#"
        "ui.background" = { bg = "black" }
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "green" }
        "ui.cursor" = { bg = "yellow" }
        "ui.virtual.fold" = { fg = "cyan" }
        "keyword" = { fg = "red" }
        "function" = { fg = "yellow" }
    "#,
        )?))?;
    keys(&mut app, ":set-language rust<ret>zfc").await?;
    let (view, doc) = current_ref!(app.editor);
    assert!(doc.syntax().is_some());
    assert_eq!(doc.folded_ranges(view.id).len(), 1);
    let area = view.inner_area(doc);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    assert_eq!(buffer[(area.x + 13, area.y)].symbol(), "…");
    assert_eq!(buffer[(area.x + 13, area.y)].fg, tui::style::Color::Cyan);
    assert_eq!(buffer[(area.x, area.y + 1)].symbol(), "f");
    assert_eq!(buffer[(area.x, area.y + 1)].fg, tui::style::Color::Red);
    assert_eq!(buffer[(area.x + 3, area.y + 1)].symbol(), "v");
    assert_eq!(
        buffer[(area.x + 3, area.y + 1)].fg,
        tui::style::Color::Yellow
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn folding_is_independent_between_splits_of_the_same_document() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[header\nhidden\nend|]#\nnext\n")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    keys(&mut app, "zfs").await?;
    let (view, doc) = current_ref!(app.editor);
    let (first, id) = (view.id, doc.id());
    app.editor.switch(id, Action::VerticalSplit);
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_ne!(view.id, first);
    assert!(doc.folded_ranges(view.id).is_empty());
    assert_eq!(doc.folded_ranges(first).len(), 1);
    let first_area = app.editor.tree.get(first).inner_area(doc);
    let second_area = view.inner_area(doc);
    let backend = app.terminal_backend();
    assert_eq!(
        backend.buffer()[(first_area.x, first_area.y + 1)].symbol(),
        "n"
    );
    assert_eq!(
        backend.buffer()[(second_area.x, second_area.y + 1)].symbol(),
        "h"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn word_jump_labels_skip_hidden_words_without_exhausting_the_alphabet() -> anyhow::Result<()>
{
    let mut config = super::helpers::test_config();
    config.editor.jump_label_alphabet = vec!['a', 'b'];
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(format!(
            "#[header\n{}end|]#\nnext word\n",
            "many hidden words\n".repeat(100)
        ))
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    keys(&mut app, "zfsgwaa").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.cursor_line(view.id), 102);
    assert_eq!(doc.folded_ranges(view.id).len(), 1);
    assert!(app.close().await.is_empty());
    Ok(())
}
