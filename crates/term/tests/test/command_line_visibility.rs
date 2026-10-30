use editor_config::{BufferLine, CommandLineShow, StatusLineElement};
use term::application::Application;
use theme::Theme;
use tui::{backend::Backend, buffer::Cell, style::Color};
use view::{current_ref, editor::Action, graphics::Rect};

use super::helpers::{keys, test_config, AppBuilder};

fn bottom_row(app: &Application) -> Vec<Cell> {
    let buffer = app.terminal_backend().buffer();
    let y = buffer.area.bottom() - 1;
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].clone())
        .collect()
}

fn bottom_text(app: &Application) -> String {
    bottom_row(app).iter().map(Cell::symbol).collect()
}

fn view_areas(app: &Application) -> Vec<Rect> {
    app.editor.tree.views().map(|(view, _)| view.area).collect()
}

fn theme() -> Theme {
    Theme::from(
        toml::from_str::<toml::Value>(
            r#"
            "ui.background" = { bg = "black" }
            "ui.text" = { fg = "white" }
            "ui.selection" = { bg = "green" }
            "ui.cursor" = { bg = "yellow" }
            "ui.statusline" = { bg = "blue" }
            "ui.statusline.inactive" = { bg = "blue" }
            "error" = { fg = "red" }
            "warning" = { fg = "yellow" }
            "#,
        )
        .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_command_line_reclaims_a_row_and_restores_statuslines() -> anyhow::Result<()> {
    for global in [false, true] {
        for bufferline in [BufferLine::Never, BufferLine::Always] {
            for split in [
                None,
                Some(Action::HorizontalSplit),
                Some(Action::VerticalSplit),
            ] {
                let mut config = test_config();
                config.editor.command_line.show = CommandLineShow::Auto;
                config.editor.statusline.global = global;
                config.editor.statusline.left = vec![StatusLineElement::Mode];
                config.editor.statusline.right = vec![StatusLineElement::Position];
                config.editor.bufferline.show = bufferline.clone();
                let mut app = AppBuilder::new()
                    .with_config(config)
                    .with_lang_loader(language::syntax::Loader::default())
                    .build()?;
                app.editor.theme = theme();
                if let Some(action) = split {
                    app.editor.new_file(action);
                }
                keys(&mut app, "<esc>").await?;
                let areas = view_areas(&app);
                let buffer = app.terminal_backend().buffer();
                let bottom = buffer.area.bottom();
                let editor_bottom = areas.iter().copied().map(Rect::bottom).max().unwrap();
                assert_eq!(editor_bottom, bottom - u16::from(global));
                let statusline = bottom_row(&app);
                // A vertical split may retain its separator between the two statuslines.
                assert_eq!(statusline.first().unwrap().bg, Color::Blue);
                assert_eq!(statusline.last().unwrap().bg, Color::Blue);

                // Messages replace the statusline without leaving text or styling behind.
                for severity in [
                    diagnostics::Severity::Info,
                    diagnostics::Severity::Warning,
                    diagnostics::Severity::Error,
                ] {
                    app.editor.status_msg = Some(("message".into(), severity));
                    app.editor.needs_redraw = true;
                    keys(&mut app, "").await?;
                    assert_eq!(bottom_text(&app).trim(), "message");
                    assert!(bottom_row(&app).iter().all(|cell| cell.bg == Color::Black));
                    assert_eq!(view_areas(&app), areas);
                    keys(&mut app, "<esc>").await?;
                    assert!(bottom_row(&app) == statusline);
                }

                // Both prompt kinds share the same last row and retain cursor ownership.
                for (input, expected) in
                    [(":echo hello", ":echo hello"), ("/hello", "search:hello")]
                {
                    keys(&mut app, input).await?;
                    assert!(
                        bottom_text(&app).starts_with(expected),
                        "prompt {input:?}: {:?}",
                        bottom_text(&app)
                    );
                    let mut backend = app.terminal_backend().clone();
                    assert!(backend.cursor_visible());
                    let cursor = backend.get_cursor_position()?;
                    assert_eq!(cursor.y, bottom - 1);
                    assert_eq!(cursor.x as usize, expected.len());
                    assert_eq!(view_areas(&app), areas);
                    keys(&mut app, "<esc>").await?;
                    assert!(bottom_row(&app) == statusline);
                }
                assert!(app.close().await.is_empty());
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_command_line_keeps_pending_keys_counts_and_macro_recording_visible(
) -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.command_line.show = CommandLineShow::Auto;
    config.editor.auto_info = false;
    config.editor.statusline.left = vec![StatusLineElement::Mode];
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    app.editor.theme = theme();
    keys(&mut app, "<esc>").await?;
    let statusline = bottom_row(&app);
    for (input, expected) in [("12", "12"), ("g", "g"), ("f", "f")] {
        keys(&mut app, input).await?;
        let text = bottom_text(&app);
        assert!(text.starts_with(" NOR"));
        assert!(text.trim_end().ends_with(expected), "{text:?}");
        keys(&mut app, "<esc>").await?;
        assert!(bottom_row(&app) == statusline);
    }
    keys(&mut app, "Q").await?;
    assert!(bottom_text(&app).ends_with("[@]"));
    keys(&mut app, "Q").await?;
    assert!(app.editor.macro_recording.is_none());
    keys(&mut app, "<esc>").await?;
    assert!(bottom_row(&app) == statusline);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_command_line_keeps_workspace_trust_indicator_visible() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join(".mitos"))?;
    std::fs::write(dir.path().join(".mitos/config.toml"), "[editor]\n")?;
    let file = dir.path().join("document.txt");
    std::fs::write(&file, "text\n")?;
    let mut config = test_config();
    config.editor.command_line.show = CommandLineShow::Auto;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(language::syntax::Loader::default())
        .with_file(file, None)
        .build()?;
    app.editor.theme = theme();
    app.editor.workspace_trust = workspace::trust::WorkspaceTrust::new(
        workspace::trust::Config {
            level: workspace::trust::ImplicitTrustLevel::None,
            prompt: false,
            ..Default::default()
        },
        dir.path().join("trust"),
    );
    keys(&mut app, "<esc>").await?;
    assert!(bottom_text(&app).ends_with("[⚠]"));
    assert_eq!(bottom_row(&app).first().unwrap().bg, Color::Blue);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_command_line_preserves_statusline_below_help_and_pickers() -> anyhow::Result<()> {
    for global in [false, true] {
        let mut config = test_config();
        config.editor.command_line.show = CommandLineShow::Auto;
        config.editor.statusline.global = global;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(language::syntax::Loader::default())
            .build()?;
        app.editor.theme = theme();
        keys(&mut app, "<esc>").await?;
        let statusline = bottom_row(&app);
        keys(&mut app, "<space>").await?;
        assert!(app.editor.autoinfo.is_some());
        // Pending keys may cover the right side, but the statusline remains below help.
        assert!(bottom_row(&app)[..100]
            .iter()
            .all(|cell| cell.bg == Color::Blue));
        keys(&mut app, "<esc>").await?;
        for open in ["<space>b", "<space>?"] {
            keys(&mut app, open).await?;
            let backend = app.terminal_backend();
            let buffer = backend.buffer();
            assert_eq!(buffer[(119, 148)].symbol(), "╯");
            assert!(bottom_row(&app) == statusline);
            keys(&mut app, "<esc>").await?;
        }
        keys(&mut app, "/[<ret>").await?;
        assert!(bottom_row(&app) == statusline);
        keys(&mut app, "<esc>").await?;
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn command_line_visibility_can_change_at_runtime() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    keys(&mut app, "<esc>").await?;
    let (view, _) = current_ref!(app.editor);
    let original_height = view.area.height;
    keys(&mut app, ":set command-line.show auto<ret>").await?;
    assert_eq!(app.editor.config().command_line.show, CommandLineShow::Auto);
    assert_eq!(current_ref!(app.editor).0.area.height, original_height + 1);
    keys(&mut app, ":get command-line.show<ret>").await?;
    assert!(bottom_text(&app).contains("auto"));
    keys(&mut app, ":set command-line.show never<ret>").await?;
    assert_eq!(app.editor.config().command_line.show, CommandLineShow::Auto);
    assert_eq!(
        app.editor.get_status().unwrap().1,
        &diagnostics::Severity::Error
    );
    keys(&mut app, ":set command-line.show always<ret>").await?;
    assert_eq!(
        app.editor.config().command_line.show,
        CommandLineShow::Always
    );
    assert_eq!(current_ref!(app.editor).0.area.height, original_height);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[test]
fn auto_command_line_can_be_selected_in_toml() {
    let source = "[editor.command-line]\nshow = 'auto'".to_owned();
    let config = term::config::Config::load(Ok(&source), Ok(String::new())).unwrap();
    assert_eq!(config.editor.command_line.show, CommandLineShow::Auto);
}
