use editor_config::{BorderStyle, PickerLayout};
use term::{
    application::Application,
    compositor::{Component, Context},
    job::Jobs,
    ui::{overlay::overlaid, Picker, PickerColumn},
};
use tui::{backend::Backend, buffer::Buffer};
use view::{editor::ConfigEvent, graphics::Rect};

use super::helpers::{keys, test_config, AppBuilder};

fn assert_picker(app: &Application, layout: PickerLayout) {
    let backend = app.terminal_backend();
    let buffer = backend.buffer();
    let area = buffer.area;
    let expected = match layout {
        PickerLayout::Auto => Rect::new(0, 0, 120, 148),
        PickerLayout::Centered => Rect::new(6, 7, 108, 133),
    };
    assert_eq!(area, Rect::new(0, 0, 120, 150));
    assert_eq!(buffer[(expected.x, expected.y)].symbol(), "╭");
    assert_eq!(
        buffer[(expected.right() - 1, expected.bottom() - 1)].symbol(),
        "╯"
    );
    assert!(backend.cursor_visible());
    assert_eq!(
        backend.clone().get_cursor_position().unwrap(),
        (expected.x + 2, expected.y + 1).into()
    );
}

fn bottom_rows(buffer: &Buffer) -> Vec<tui::buffer::Cell> {
    (buffer.area.bottom() - 2..buffer.area.bottom())
        .flat_map(|y| (0..buffer.area.width).map(move |x| buffer[(x, y)].clone()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn pickers_share_layout_and_preserve_bottom_rows() -> anyhow::Result<()> {
    for layout in [PickerLayout::Auto, PickerLayout::Centered] {
        for global in [false, true] {
            let mut config = test_config();
            config.editor.picker.layout = layout;
            config.editor.border_style = BorderStyle::Rounded;
            config.editor.statusline.global = global;
            let mut app = AppBuilder::new()
                .with_config(config)
                .with_lang_loader(language::syntax::Loader::default())
                .build()?;
            keys(&mut app, "<esc>").await?;
            let before = bottom_rows(app.terminal_backend().buffer());
            for open in ["<space>b", "<space>?"] {
                keys(&mut app, open).await?;
                assert_picker(&app, layout);
                assert!(bottom_rows(app.terminal_backend().buffer()) == before);
                keys(&mut app, "<esc>").await?;
            }
            assert!(app.close().await.is_empty());
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn open_and_resumed_pickers_follow_live_layout_changes() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    keys(&mut app, "<space>b").await?;
    assert_picker(&app, PickerLayout::Auto);
    let mut config = (*app.editor.config()).clone();
    config.picker.layout = PickerLayout::Centered;
    app.handle_config_events(ConfigEvent::Update(Box::new(config)));
    keys(&mut app, "<home>").await?;
    assert_picker(&app, PickerLayout::Centered);
    keys(&mut app, "<esc>:set picker.layout auto<ret><space>'").await?;
    assert_picker(&app, PickerLayout::Auto);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn page_navigation_tracks_layout_changes_and_terminal_resizes() -> anyhow::Result<()> {
    let config = test_config();
    let mut app = AppBuilder::new()
        .with_config(config.clone())
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    let picker = Picker::new(
        app.editor.redraw().clone(),
        [PickerColumn::new("number", |number: &usize, _| {
            number.to_string().into()
        })],
        0,
        0..300,
        (),
        |_, _, _| {},
    );
    let mut overlay = overlaid(picker);
    let mut jobs = Jobs::new();
    let (updates, _) = tokio::sync::mpsc::unbounded_channel();
    for (layout, width, height, page_size) in [
        (PickerLayout::Auto, 120, 42, 36),
        (PickerLayout::Centered, 120, 42, 32),
        (PickerLayout::Auto, 200, 42, 32),
        (PickerLayout::Auto, 199, 42, 36),
        (PickerLayout::Centered, 120, 22, 14),
    ] {
        let mut settings = (*app.editor.config()).clone();
        settings.picker.layout = layout;
        app.handle_config_events(ConfigEvent::Update(Box::new(settings)));
        let area = Rect::new(0, 0, width, height);
        overlay.render(
            area,
            &mut Buffer::empty(area),
            &mut Context {
                config: term::config::Context {
                    current: &config,
                    updates: &updates,
                },
                editor: &mut app.editor,
                scroll: None,
                jobs: &mut jobs,
                image_picker: None,
                is_cursor_owner: true,
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while overlay.has_pending_render() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        overlay.content.to_start();
        overlay.content.page_down();
        assert_eq!(overlay.content.selection(), Some(&page_size));
    }
    assert!(app.close().await.is_empty());
    Ok(())
}
