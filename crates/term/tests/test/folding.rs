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
            Some(doc.folded_ranges(view.id)[0].range.end),
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

#[tokio::test(flavor = "multi_thread")]
async fn syntax_folds_keep_closing_tokens_and_suffixes_selectable() -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{
        Event, KeyModifiers as Modifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    #[cfg(not(windows))]
    use termina::event::{Event, Modifiers, MouseButton, MouseEvent, MouseEventKind};
    for (language, source, header_row, expected, closing) in [
        (
            "rust",
            "#[p|]#ub struct Editor {\n    value: usize,\n}; // retained\nnext();\n",
            0,
            "pub struct Editor {…}; // retained",
            '}',
        ),
        (
            "json",
            "{\n#[ |]# \"items\": [\n    1,\n    2\n  ],\n  \"next\": 3\n}\n",
            1,
            "  \"items\": […],",
            ']',
        ),
        (
            "rust",
            "#[c|]#all!(\n    value,\n); // retained\nnext();\n",
            0,
            "call!(…); // retained",
            ')',
        ),
    ] {
        let mut config = super::helpers::test_config();
        config.editor.line_number = editor_config::LineNumber::Relative;
        config.editor.cursor.highlight_line = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text(source)
            .with_lang_loader(test_syntax_loader(None))
            .build()?;
        app.editor
            .set_theme(theme::Theme::from(toml::from_str::<toml::Value>(
                r#"
            "ui.background" = { bg = "black" }
            "ui.text" = { fg = "white" }
            "ui.selection" = { bg = "green" }
            "ui.cursor" = { bg = "yellow" }
            "ui.cursorline.primary" = { bg = "blue" }
            "ui.linenr" = { fg = "white" }
            "ui.linenr.selected" = { fg = "magenta" }
        "#,
            )?))?;
        keys(&mut app, &format!(":set-language {language}<ret>zfc")).await?;
        let (view, doc) = current_ref!(app.editor);
        let view_id = view.id;
        let original = doc.text().clone();
        assert_eq!(doc.folded_ranges(view_id).len(), 1, "{language}");
        let area = view.inner_area(doc);
        let row = area.y + header_row;
        let backend = app.terminal_backend();
        let rendered: String = (area.x..area.right())
            .map(|x| backend.buffer()[(x, row)].symbol())
            .collect();
        assert_eq!(rendered.trim_end(), expected, "{language}");
        let closing_column = expected.chars().position(|ch| ch == '…').unwrap() as u16 + 1;
        let position = view
            .pos_at_screen_coords(doc, row, area.x + closing_column, true)
            .unwrap();
        assert_eq!(doc.text().get_char_at(position), Some(closing));
        assert_eq!(
            view.layout(doc, None).block_line_at_pos(position),
            header_row as usize
        );
        let source_line = doc.text().char_to_line_idx(position);
        assert!(source_line > header_row as usize);
        app.handle_terminal_events(Ok(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            row,
            column: area.x + closing_column,
            modifiers: Modifiers::empty(),
        })))
        .await;
        keys(&mut app, "").await?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(doc.folded_ranges(view_id).len(), 1);
        let backend = app.terminal_backend();
        assert_eq!(backend.buffer()[(area.x, row)].bg, tui::style::Color::Blue);
        assert!((view.area.x..area.x).any(|x| {
            let cell = &backend.buffer()[(x, row)];
            cell.symbol() == (header_row + 1).to_string() && cell.fg == tui::style::Color::Magenta
        }));
        keys(&mut app, "j").await?;
        let (_, doc) = current_ref!(app.editor);
        assert_eq!(doc.cursor_line(view_id), source_line + 1);
        assert_eq!(doc.folded_ranges(view_id).len(), 1);
        assert_eq!(doc.text(), &original);
        keys(&mut app, "k").await?;
        let (_, doc) = current_ref!(app.editor);
        assert_eq!(
            doc.selection(view_id)
                .primary()
                .cursor(doc.text().slice(..)),
            position
        );
        keys(&mut app, "aX<esc>").await?;
        let (_, doc) = current_ref!(app.editor);
        let mut edited = original.to_string();
        edited.insert(original.char_to_byte_idx(position + 1), 'X');
        assert_eq!(doc.text().to_string(), edited);
        assert_eq!(doc.folded_ranges(view_id).len(), 1);
        keys(&mut app, "zfo").await?;
        let (view, doc) = current_ref!(app.editor);
        assert!(doc.folded_ranges(view.id).is_empty());
        assert!(app.close().await.is_empty());
    }
    Ok(())
}
