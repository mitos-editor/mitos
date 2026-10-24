use editor_config::{BufferLine, RulerStyle, WhitespaceRender, WhitespaceRenderValue};
use term::application::Application;
use theme::Theme;
use tui::style::Color;
use view::{current_ref, editor::Action};

use super::helpers::{keys, test_config, test_syntax_loader, AppBuilder};

fn ruler_theme() -> Theme {
    Theme::from(
        toml::from_str::<toml::Value>(
            r#"
        "ui.background" = { bg = "black" }
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "green" }
        "ui.cursor" = { bg = "yellow" }
        "ui.virtual.ruler" = { fg = "red", bg = "blue" }
        "#,
        )
        .unwrap(),
    )
}

fn assert_rulers(app: &Application, columns: &[u16], character: &str) {
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    let mut count = 0;
    for (view, _) in app.editor.tree.views() {
        let doc = app.editor.document(view.document_id()).unwrap();
        let area = view.inner_area(doc);
        let offset = doc.view_offset(view.id).horizontal_offset;
        for column in columns {
            let Some(column) = usize::from(*column)
                .checked_sub(1)
                .and_then(|col| col.checked_sub(offset))
            else {
                continue;
            };
            if column >= usize::from(area.width) {
                continue;
            }
            let x = area.x + column as u16;
            // These fixtures have at most one line of text. Check the full
            // remaining height, including the last content row of each split.
            for y in area.y + 1..area.bottom() {
                assert_eq!(
                    buffer[(x, y)].symbol(),
                    character,
                    "at ({x}, {y}) in {area:?}"
                );
                assert_eq!(buffer[(x, y)].fg, Color::Red);
                assert_eq!(buffer[(x, y)].bg, Color::Black);
                count += 1;
            }
        }
    }
    // Rulers must not leak into gutters, pane borders, bufferlines or statuslines.
    let total = buffer
        .content()
        .iter()
        .filter(|cell| cell.symbol() == character)
        .count();
    assert_eq!(total, count);
}

#[tokio::test(flavor = "multi_thread")]
async fn glyph_rulers_show_through_hidden_whitespace() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.rulers.columns = vec![3, 4, 5];
    config.editor.rulers.style = RulerStyle::Glyph;
    config.editor.whitespace.render = WhitespaceRender::Basic(WhitespaceRenderValue::None);
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[x|]#   x\nx\t x\nxx\nxx\u{00a0}\u{202f}x\nx界x\n")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    app.editor.theme = ruler_theme();
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    let area = view.inner_area(doc);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    for (row, expected) in ["┊┊x", "┊┊┊", "┊┊┊", "┊┊x"].into_iter().enumerate()
    {
        for (column, symbol) in expected.chars().enumerate() {
            let cell = &buffer[(area.x + 2 + column as u16, area.y + row as u16)];
            assert_eq!(
                cell.symbol(),
                symbol.to_string(),
                "row {row}, column {column}"
            );
            assert_eq!(
                cell.fg,
                if symbol == 'x' {
                    Color::White
                } else {
                    Color::Red
                }
            );
            assert_eq!(cell.bg, Color::Black);
        }
    }
    // A wide character clears the ruler in its continuation cell as well.
    assert_eq!(buffer[(area.x + 1, area.y + 4)].symbol(), "界");
    assert_eq!(buffer[(area.x + 2, area.y + 4)].symbol(), " ");
    assert_eq!(buffer[(area.x + 3, area.y + 4)].symbol(), "x");
    assert_eq!(buffer[(area.x + 4, area.y + 4)].symbol(), "┊");

    keys(&mut app, ":set whitespace.render all<ret>").await?;
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    for (row, expected) in ["··x", "┊┊·", "⏎┊┊", "⍽␣x"].into_iter().enumerate() {
        for (column, symbol) in expected.chars().enumerate() {
            assert_eq!(
                buffer[(area.x + 2 + column as u16, area.y + row as u16)].symbol(),
                symbol.to_string(),
                "row {row}, column {column}"
            );
        }
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn glyph_rulers_preserve_whitespace_highlights() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.rulers.columns = vec![3, 4];
    config.editor.rulers.style = RulerStyle::Glyph;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[x   |]#x")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    app.editor.theme = ruler_theme();
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    let area = view.inner_area(doc);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    for (column, background) in [(2, Color::Green), (3, Color::Yellow)] {
        let cell = &buffer[(area.x + column, area.y)];
        assert_eq!(cell.symbol(), "┊");
        assert_eq!(cell.fg, Color::Red);
        assert_eq!(cell.bg, background);
    }
    // An explicit cursor foreground still takes precedence over the ruler.
    app.editor.theme = Theme::from(toml::from_str::<toml::Value>(
        r#"
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "green" }
        "ui.cursor" = { fg = "black", bg = "yellow" }
        "ui.virtual.ruler" = { fg = "red" }
        "#,
    )?);
    keys(&mut app, "<esc>").await?;
    let backend = app.terminal_backend();
    let cell = &backend.buffer()[(area.x + 3, area.y)];
    assert_eq!(cell.symbol(), "┊");
    assert_eq!(cell.fg, Color::Black);
    assert_eq!(cell.bg, Color::Yellow);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn glyph_rulers_show_through_clipped_tabs() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.gutters.layout.clear();
    config.editor.rulers.columns = vec![2, 3, 122];
    config.editor.rulers.style = RulerStyle::Glyph;
    config.editor.whitespace.render = WhitespaceRender::Basic(WhitespaceRenderValue::None);
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(format!(
            "x\t{}\tx\n{}#[x|]#\n",
            "x".repeat(117),
            "x".repeat(70)
        ))
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    app.editor.theme = ruler_theme();
    {
        let (view, doc) = view::current!(app.editor);
        doc.set_view_offset(
            view.id,
            view::view::ViewPosition {
                horizontal_offset: 2,
                ..Default::default()
            },
        );
    }
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    let area = view.inner_area(doc);
    let offset = doc.view_offset(view.id).horizontal_offset;
    assert_eq!(offset, 2);
    assert_eq!(area.width, 120);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    // Both viewport edges cut through a tab; the visible pads retain rulers.
    for x in [area.x, area.right() - 1] {
        assert_eq!(buffer[(x, area.y)].symbol(), "┊");
        assert_eq!(buffer[(x, area.y)].fg, Color::Red);
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rulers_cover_split_content_and_preserve_text() -> anyhow::Result<()> {
    for global in [false, true] {
        for bufferline in [BufferLine::Never, BufferLine::Always] {
            for split in [
                None,
                Some(Action::HorizontalSplit),
                Some(Action::VerticalSplit),
            ] {
                let mut config = test_config();
                config.editor.rulers.columns = vec![0, 3, u16::MAX];
                config.editor.rulers.style = RulerStyle::Glyph;
                config.editor.bufferline.show = bufferline.clone();
                config.editor.statusline.global = global;
                let mut app = AppBuilder::new()
                    .with_config(config)
                    .with_input_text("#[a|]#bcdef\n")
                    .with_lang_loader(language::syntax::Loader::default())
                    .build()?;
                app.editor.theme = ruler_theme();
                if let Some(action) = split {
                    let doc = current_ref!(app.editor).1.id();
                    app.editor.switch(doc, action);
                }
                keys(&mut app, "<esc>").await?;
                assert_rulers(&app, &[0, 3, u16::MAX], "┊");
                for (view, _) in app.editor.tree.views() {
                    let doc = app.editor.document(view.document_id()).unwrap();
                    let area = view.inner_area(doc);
                    assert_eq!(
                        app.terminal_backend().buffer()[(area.x + 2, area.y)].symbol(),
                        "c"
                    );
                }
                assert!(app.close().await.is_empty());
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ruler_runtime_updates_keep_legacy_column_commands() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.rulers.columns = vec![3];
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[a|]#bcdef\n")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    app.editor.theme = ruler_theme();
    keys(&mut app, ":set rulers.style glyph<ret>").await?;
    assert_rulers(&app, &[3], "┊");
    keys(&mut app, ":set rulers.character |<ret>").await?;
    keys(&mut app, ":set rulers [4]<ret>").await?;
    assert_eq!(app.editor.config().rulers.character, '|');
    assert_eq!(app.editor.config().rulers.columns, [4]);
    assert_rulers(&app, &[4], "|");
    keys(&mut app, ":toggle rulers.columns [4] [5]<ret>").await?;
    assert_rulers(&app, &[5], "|");
    keys(&mut app, ":set rulers.style background<ret>").await?;
    assert_eq!(app.editor.config().rulers.style, RulerStyle::Background);
    let (view, doc) = current_ref!(app.editor);
    let area = view.inner_area(doc);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    assert_eq!(buffer[(area.x + 4, area.y)].symbol(), "e");
    assert_eq!(buffer[(area.x + 4, area.y)].fg, Color::White);
    assert_eq!(buffer[(area.x + 4, area.y)].bg, Color::Blue);
    assert_eq!(buffer[(area.x + 4, area.bottom() - 1)].symbol(), " ");
    keys(&mut app, ":set rulers.style glyph<ret>").await?;
    assert_rulers(&app, &[5], "|");
    keys(&mut app, ":set rulers.style line<ret>").await?;
    assert_eq!(app.editor.config().rulers.style, RulerStyle::Glyph);
    assert_rulers(&app, &[5], "|");
    keys(&mut app, ":set rulers.character 界<ret>").await?;
    assert_eq!(app.editor.config().rulers.character, '|');
    assert_rulers(&app, &[5], "|");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_rulers_override_editor_columns_including_empty_lists() -> anyhow::Result<()> {
    for columns in ["[5]", "[]"] {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("test.ruler-test");
        std::fs::write(&file, "abcdef\n")?;
        let loader = test_syntax_loader(Some(format!(
            r#"
            [[language]]
            name = "ruler-test"
            scope = "source.ruler-test"
            file-types = ["ruler-test"]
            rulers = {columns}
        "#
        )));
        let mut config = test_config();
        config.editor.rulers.columns = vec![3];
        config.editor.rulers.style = RulerStyle::Glyph;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_file(file, None)
            .with_lang_loader(loader)
            .build()?;
        app.editor.theme = ruler_theme();
        keys(&mut app, "<esc>").await?;
        assert!(current_ref!(app.editor).1.language_config().is_some());
        assert_rulers(&app, if columns == "[]" { &[] } else { &[5] }, "┊");
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rulers_follow_horizontal_scrolls_without_offset_truncation() -> anyhow::Result<()> {
    for length in [150, 70_000] {
        let mut config = test_config();
        config.editor.rulers.columns = vec![0, 1, 80, 100, u16::MAX];
        config.editor.rulers.style = RulerStyle::Glyph;
        let input = format!("{}#[x|]#\n", "x".repeat(length));
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text(input)
            .with_lang_loader(language::syntax::Loader::default())
            .build()?;
        app.editor.theme = ruler_theme();
        keys(&mut app, "<esc>").await?;
        let (view, doc) = current_ref!(app.editor);
        let offset = doc.view_offset(view.id).horizontal_offset;
        if length > usize::from(u16::MAX) {
            assert!(offset > usize::from(u16::MAX));
        } else {
            assert!(offset > 0 && offset < 80);
        }
        assert_rulers(&app, &[0, 1, 80, 100, u16::MAX], "┊");
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn default_rulers_preserve_background_styling_across_text() -> anyhow::Result<()> {
    for ruler_style in [
        "",
        r#""ui.virtual.ruler" = { bg = "red" }"#,
        r#""ui.virtual.ruler" = { bg = "red", fg = "blue", modifiers = ["bold"] }"#,
    ] {
        let mut config = test_config();
        config.editor.rulers.columns = vec![3];
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text("#[a|]#bcdef\n")
            .with_lang_loader(language::syntax::Loader::default())
            .build()?;
        app.editor.theme = Theme::from(toml::from_str::<toml::Value>(&format!(
            r#"
            "ui.text" = {{ fg = "white" }}
            "ui.selection" = {{ bg = "green" }}
            "ui.cursor" = {{ bg = "yellow" }}
            {ruler_style}
        "#,
        ))?);
        keys(&mut app, "<esc>").await?;
        let (view, doc) = current_ref!(app.editor);
        let area = view.inner_area(doc);
        let backend = app.terminal_backend();
        let buffer = backend.buffer();
        for y in area.top()..area.bottom() {
            assert_eq!(buffer[(area.x + 2, y)].bg, Color::Red);
        }
        assert_eq!(buffer[(area.x + 2, area.y)].symbol(), "c");
        assert_eq!(buffer[(area.x + 2, area.y)].fg, Color::White);
        assert!(!buffer[(area.x + 2, area.y)]
            .modifier
            .contains(tui::style::Modifier::BOLD));
        assert_eq!(buffer[(area.x + 2, area.bottom() - 1)].symbol(), " ");
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn glyph_rulers_use_foreground_with_background_and_default_fallbacks() -> anyhow::Result<()> {
    for ruler_style in [
        "",
        r#""ui.virtual.ruler" = { bg = "red" }"#,
        r#""ui.virtual.ruler" = { fg = "red" }"#,
        r#""ui.virtual.ruler" = { fg = "red", bg = "blue" }"#,
    ] {
        let mut config = test_config();
        config.editor.rulers.columns = vec![3];
        config.editor.rulers.style = RulerStyle::Glyph;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text("#[a|]#bcdef\n")
            .with_lang_loader(language::syntax::Loader::default())
            .build()?;
        app.editor.theme = Theme::from(toml::from_str::<toml::Value>(&format!(
            r#"
            "ui.background" = {{ bg = "black" }}
            "ui.text" = {{ fg = "white" }}
            "ui.selection" = {{ bg = "green" }}
            "ui.cursor" = {{ bg = "yellow" }}
            {ruler_style}
        "#,
        ))?);
        keys(&mut app, "<esc>").await?;
        assert_rulers(&app, &[3], "┊");
        let (view, doc) = current_ref!(app.editor);
        let area = view.inner_area(doc);
        let backend = app.terminal_backend();
        let buffer = backend.buffer();
        assert_eq!(buffer[(area.x + 2, area.y)].symbol(), "c");
        assert_eq!(buffer[(area.x + 2, area.y)].fg, Color::White);
        assert_eq!(buffer[(area.x + 2, area.y)].bg, Color::Black);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cursorline_does_not_recolor_gutters() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.cursor.highlight_line = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[|a]#bc")
        .build()?;
    app.editor.theme = Theme::from(toml::from_str::<toml::Value>(
        r#"
        "ui.background" = { bg = "black" }
        "ui.cursorline.primary" = { bg = "red" }
        "ui.selection" = { bg = "green" }
        "ui.cursor" = { bg = "yellow" }
    "#,
    )?);
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    let inner = view.inner_area(doc);
    let backend = app.terminal_backend();
    assert_eq!(backend.buffer()[(inner.x + 2, inner.y)].bg, Color::Red);
    assert_ne!(backend.buffer()[(inner.x - 1, inner.y)].bg, Color::Red);
    Ok(())
}
