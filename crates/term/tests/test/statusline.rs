use editor_config::{BufferLine, StatusLineElement};
use term::application::Application;
use theme::Theme;
use tui::{buffer::Buffer, style::Color};
use view::{current_ref, editor::Action, graphics::Rect};

use super::helpers::{test_config, test_key_sequences, AppBuilder};

fn row(buffer: &Buffer, area: Rect) -> String {
    (area.x..area.right())
        .map(|x| buffer[(x, area.y)].symbol())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn search_position_renders_cached_exact_capped_and_noncanonical_counts() -> anyhow::Result<()>
{
    use super::helpers::{callbacks, keys};
    use editor_config::OptionToml;
    use editor_core::search::statistics::Position;
    use view::features::buffer_search::Handler;

    for (text, prefix, input, limit, expected) in [
        ("#[o|]#ne one one\n", "/o", "ne<ret>", Some(100), "[2/3]"),
        ("#[o|]#ne one one\n", "/o", "ne<ret>", Some(2), "[2/>2]"),
        ("#[o|]#ne one one\n", "/o", "ne<ret>", Some(1), "[>1/>1]"),
        ("#[a|]#aa\n", "/a", "a<ret>", None, "[?/1]"),
        ("aa#[a|]#\n", "?a", "+<ret>", None, "[?/1]"),
    ] {
        let (sender, mut updates) = callbacks::channel();
        let mut config = test_config();
        config.editor.statusline.left.clear();
        config.editor.statusline.right = vec![StatusLineElement::SearchPosition];
        config.editor.search.max_matches = limit.map_or(OptionToml::None, OptionToml::Some);
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(language::syntax::Loader::default())
            .with_input_text(text)
            .with_handler_setup(move |handlers, _| handlers.buffer_search = Handler::new(sender))
            .build()?;
        // Force a prefix result to be queued before the final query replaces it.
        keys(&mut app, prefix).await?;
        let obsolete_update =
            tokio::time::timeout(std::time::Duration::from_secs(5), updates.recv())
                .await?
                .unwrap();
        keys(&mut app, input).await?;
        obsolete_update(&mut app.editor);
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(doc.search_position(view.id), Some(Position::PENDING));

        // The first callback may be obsolete. Wait for the current query's
        // publication before testing the rendered count or cached navigation.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let (view, doc) = current_ref!(app.editor);
                if doc.search_position(view.id) != Some(Position::PENDING) {
                    break;
                }
                updates.recv().await.expect("search callback queue closed")(&mut app.editor);
            }
        })
        .await?;
        keys(&mut app, "").await?;
        let (view, doc) = current_ref!(app.editor);
        let rendered = row(app.terminal_backend().buffer(), view.statusline_area(doc));
        assert!(
            rendered.contains(expected),
            "query {prefix}{input}: expected {expected:?}, got {rendered:?}"
        );

        // Navigation reuses the cache even if other obsolete callbacks are queued.
        keys(&mut app, "n").await?;
        let (view, doc) = current_ref!(app.editor);
        assert!(doc
            .search_position(view.id)
            .is_some_and(|position| position != Position::PENDING));

        // Ordinary movement clears the displayed position.
        keys(&mut app, "h").await?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(doc.search_position(view.id), None);
        assert!(!row(app.terminal_backend().buffer(), view.statusline_area(doc)).contains('['));
    }
    Ok(())
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

#[tokio::test(flavor = "multi_thread")]
async fn key_help_preserves_the_bottom_statusline() -> anyhow::Result<()> {
    for global in [false, true] {
        for bufferline in [BufferLine::Never, BufferLine::Always] {
            for split in [
                None,
                Some(Action::HorizontalSplit),
                Some(Action::VerticalSplit),
            ] {
                let mut config = test_config();
                config.editor.auto_info = true;
                config.editor.statusline.global = global;
                config.editor.bufferline.show = bufferline.clone();
                let mut app = AppBuilder::new()
                    .with_config(config)
                    .with_lang_loader(language::syntax::Loader::default())
                    .build()?;
                app.editor.theme = Theme::from(toml::from_str::<toml::Value>(
                    r#"
                    "ui.text" = { fg = "white" }
                    "ui.selection" = { bg = "green" }
                    "ui.cursor" = { bg = "yellow" }
                    "ui.statusline" = { bg = "blue" }
                    "ui.statusline.inactive" = { bg = "gray" }
                    "ui.popup.info" = { bg = "red" }
                    "#,
                )?);
                if let Some(action) = split {
                    app.editor.new_file(action);
                }
                let statusline = std::cell::RefCell::new(Vec::new());
                let remember_statusline = |app: &Application| {
                    let backend = app.terminal_backend();
                    let buffer = backend.buffer();
                    let y = buffer.area.bottom() - 2;
                    *statusline.borrow_mut() = (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].clone())
                        .collect::<Vec<_>>();
                };
                let assert_help = |app: &Application| {
                    assert!(app.editor.autoinfo.is_some());
                    let backend = app.terminal_backend();
                    let buffer = backend.buffer();
                    let y = buffer.area.bottom() - 2;
                    let current: Vec<_> = (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].clone())
                        .collect();
                    assert!(current == *statusline.borrow(), "help covered the statusline (global={global}, bufferline={bufferline:?}, split={split:?})");
                    // Ensure the popup was actually painted, rather than merely queued.
                    assert!(buffer.content().iter().any(|cell| cell.bg == Color::Red));
                    assert!((0..buffer.area.width).any(|x| buffer[(x, y - 1)].bg == Color::Red));
                };
                test_key_sequences(
                    &mut app,
                    vec![
                        (Some("<esc>"), Some(&remember_statusline)),
                        (Some("<space>"), Some(&assert_help)),
                        (Some("<esc>:qa!<ret>"), None),
                    ],
                    true,
                )
                .await?;
            }
        }
    }
    Ok(())
}
