use term::application::Application;
use theme::Theme;
use tui::{buffer::Buffer, style::Color};
use view::{
    config::{BufferLine, StatusLineElement},
    current_ref,
    editor::Action,
    graphics::Rect,
};

use super::helpers::{test_config, test_key_sequences, AppBuilder};

fn row(buffer: &Buffer, area: Rect) -> String {
    (area.x..area.right())
        .map(|x| buffer[(x, area.y)].symbol())
        .collect()
}

fn assert_per_view(app: &Application) {
    assert!(!app.editor.config().statusline.global);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    for (view, focused) in app.editor.tree.views() {
        let doc = app.editor.document(view.document_id()).unwrap();
        let area = view.statusline_area(doc);
        let text = row(buffer, area);
        assert!(text.contains(doc.path().unwrap().file_name().unwrap().to_str().unwrap()));
        assert_eq!(text.contains("NOR"), focused);
        assert_eq!(
            buffer[(area.x, area.y)].bg,
            if focused {
                Color::Blue
            } else {
                Color::DarkGray
            }
        );
        assert_eq!(view.inner_area(doc).bottom(), area.y);
    }
    let cursor = app.editor.cursor().0.unwrap();
    let (view, doc) = current_ref!(app.editor);
    assert!((cursor.row as u16) < view.statusline_area(doc).y);
}

fn assert_global(app: &Application) {
    assert!(app.editor.config().statusline.global);
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    let area = Rect::new(0, buffer.area.bottom() - 2, buffer.area.width, 1);
    let text = row(buffer, area);
    let (_, doc) = current_ref!(app.editor);
    let filename = doc.path().unwrap().file_name().unwrap().to_str().unwrap();
    let other_filename = if filename == "first.txt" {
        "second.txt"
    } else {
        "first.txt"
    };
    assert!(text.contains(filename));
    assert!(!text.contains(other_filename));
    assert!(text.contains("NOR"));
    assert_eq!(buffer[(area.x, area.y)].bg, Color::Blue);
    for (view, _) in app.editor.tree.views() {
        let doc = app.editor.document(view.document_id()).unwrap();
        assert!(view.statusline_area(doc).is_empty());
        assert_eq!(view.content_area(doc), view.area);
    }
    let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert_eq!(screen.matches(other_filename).count(), 0);
    assert_eq!(screen.matches(filename).count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn split_statuslines_default_to_each_view_and_can_toggle_global() -> anyhow::Result<()> {
    for action in [Action::HorizontalSplit, Action::VerticalSplit] {
        let dir = tempfile::tempdir()?;
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, "first\n")?;
        std::fs::write(&second, "second\n")?;
        let mut config = test_config();
        config.editor.bufferline.show = BufferLine::Never;
        config.editor.statusline.left =
            vec![StatusLineElement::Mode, StatusLineElement::FileBaseName];
        config.editor.statusline.right = vec![StatusLineElement::Position];
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_file(first, None)
            .with_lang_loader(language::syntax::Loader::default())
            .build()?;
        app.editor.theme = Theme::from(toml::from_str::<toml::Value>(
            r#"
            "ui.text" = { fg = "white" }
            "ui.selection" = { bg = "green" }
            "ui.cursor" = { bg = "yellow" }
            "ui.statusline" = { bg = "blue" }
            "ui.statusline.inactive" = { bg = "gray" }
        "#,
        )?);
        app.editor.open(&second, action)?;
        test_key_sequences(
            &mut app,
            vec![
                (Some("<esc>"), Some(&assert_per_view)),
                (Some(":toggle statusline.global<ret>"), Some(&assert_global)),
                (Some("<C-w>w"), Some(&assert_global)),
                (
                    Some(":toggle statusline.global<ret>"),
                    Some(&assert_per_view),
                ),
                (Some(":qa!<ret>"), None),
            ],
            true,
        )
        .await?;
    }
    Ok(())
}

#[test]
fn global_statusline_can_be_selected_in_toml() {
    let source = "[editor.statusline]\nglobal = true".to_owned();
    let config = term::config::Config::load(Ok(&source), Ok(String::new())).unwrap();
    assert!(config.editor.statusline.global);
    assert!(!term::config::Config::default().editor.statusline.global);
}
