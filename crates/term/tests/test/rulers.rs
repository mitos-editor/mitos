use editor_config::BufferLine;
use term::application::Application;
use theme::Theme;
use tui::style::Color;
use view::{current_ref, editor::Action};

use super::helpers::{keys, test_config, test_syntax_loader, AppBuilder};

fn ruler_theme() -> Theme {
    Theme::from(
        toml::from_str::<toml::Value>(
            r#"
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "green" }
        "ui.cursor" = { bg = "yellow" }
        "ui.virtual.ruler" = { fg = "red" }
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
    keys(&mut app, ":set rulers.character |<ret>").await?;
    keys(&mut app, ":set rulers [4]<ret>").await?;
    assert_eq!(app.editor.config().rulers.character, '|');
    assert_eq!(app.editor.config().rulers.columns, [4]);
    assert_rulers(&app, &[4], "|");
    keys(&mut app, ":toggle rulers.columns [4] [5]<ret>").await?;
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
async fn space_rulers_preserve_background_styling_across_text() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.rulers.columns = vec![3];
    config.editor.rulers.character = ' ';
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[a|]#bcdef\n")
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    app.editor.theme = Theme::from(toml::from_str::<toml::Value>(
        r#"
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "green" }
        "ui.cursor" = { bg = "yellow" }
        "ui.virtual.ruler" = { bg = "red" }
    "#,
    )?);
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    let area = view.inner_area(doc);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    for y in area.top()..area.bottom() {
        assert_eq!(buffer[(area.x + 2, y)].bg, Color::Red);
    }
    assert_eq!(buffer[(area.x + 2, area.y)].symbol(), "c");
    assert_eq!(buffer[(area.x + 2, area.bottom() - 1)].symbol(), " ");
    assert!(app.close().await.is_empty());
    Ok(())
}
