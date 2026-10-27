use super::test::{application, draw, frame, until};
use super::*;
use crate::{application::Application, config::Config as AppConfig, job::Jobs};

/// Send one key through the picker's real event handling, then redraw like the editor does.
fn press(picker: &mut Picker<u32, ()>, app: &mut Application, key: &str) {
    let config = AppConfig::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = Jobs::new();
    let mut cx = Context {
        config: crate::config::Context {
            current: &config,
            updates: &updates,
        },
        editor: &mut app.editor,
        jobs: &mut jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    picker.handle_event(&Event::Key(key.parse().unwrap()), &mut cx);
    draw(picker, app);
}

/// Screen row showing `item`, read from the rendered frame.
fn screen_row(picker: &mut Picker<u32, ()>, app: &mut Application, item: u32) -> usize {
    let surface = frame(picker, app);
    let label = format!("result-{item:03}");
    surface
        .content
        .chunks(surface.area.width as usize)
        .position(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains(&label)
        })
        .unwrap_or_else(|| panic!("{label} is not on screen"))
}

async fn picker(app: &mut Application, len: u32) -> anyhow::Result<Picker<u32, ()>> {
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("name", |item: &u32, _: &()| {
            format!("result-{item:03}").into()
        })],
        0,
        0..len,
        (),
        |_, _, _| {},
    );
    until(&mut picker, app, |picker| {
        picker.results.matcher.snapshot().matched_item_count() == len
    })
    .await?;
    assert!(picker.completion_height > 3, "test frame must show a page");
    Ok(picker)
}

#[tokio::test]
async fn page_down_keeps_a_mid_page_selection_on_its_screen_row() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = picker(&mut app, 100).await?;
    let rows = u32::from(picker.completion_height);
    for _ in 0..3 {
        press(&mut picker, &mut app, "down");
    }
    assert_eq!(picker.selection(), Some(&3));
    let row = screen_row(&mut picker, &mut app, 3);

    for key in ["pagedown", "C-d"] {
        let before = *picker.selection().unwrap();
        press(&mut picker, &mut app, key);
        // One press moves a whole page; it never stops at the bottom of the current page.
        assert_eq!(picker.selection(), Some(&(before + rows)), "{key}");
        assert_eq!(
            screen_row(&mut picker, &mut app, before + rows),
            row,
            "{key}"
        );
    }
    for key in ["pageup", "C-u"] {
        let before = *picker.selection().unwrap();
        press(&mut picker, &mut app, key);
        assert_eq!(picker.selection(), Some(&(before - rows)), "{key}");
        assert_eq!(
            screen_row(&mut picker, &mut app, before - rows),
            row,
            "{key}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn page_down_near_the_end_never_moves_the_selection_up() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = picker(&mut app, 40).await?;
    let rows = u32::from(picker.completion_height);
    for _ in 0..2 {
        press(&mut picker, &mut app, "down");
    }
    let row = screen_row(&mut picker, &mut app, 2);
    let last_page = 40 - rows;
    while picker.results.offset < last_page {
        let before = *picker.selection().unwrap();
        press(&mut picker, &mut app, "pagedown");
        let after = *picker.selection().unwrap();
        assert!(
            after > before,
            "selection moved up from {before} to {after}"
        );
        assert_eq!(screen_row(&mut picker, &mut app, after), row);
    }
    // Once scrolling stops, reach the final result and stay there.
    for key in ["pagedown", "C-d"] {
        press(&mut picker, &mut app, key);
        assert_eq!(picker.selection(), Some(&39));
        assert_eq!(picker.results.offset, last_page);
        assert_eq!(
            screen_row(&mut picker, &mut app, 39),
            row + (rows - 3) as usize
        );
    }
    // Paging up likewise reaches the first result without wrapping.
    while picker.results.offset > 0 {
        let before = *picker.selection().unwrap();
        press(&mut picker, &mut app, "pageup");
        assert!(*picker.selection().unwrap() < before);
    }
    for key in ["pageup", "C-u"] {
        press(&mut picker, &mut app, key);
        assert_eq!(picker.selection(), Some(&0));
        assert_eq!(picker.results.offset, 0);
    }
    // Individual arrow navigation keeps its existing wrap behavior.
    press(&mut picker, &mut app, "up");
    assert_eq!(picker.selection(), Some(&39));
    press(&mut picker, &mut app, "down");
    assert_eq!(picker.selection(), Some(&0));
    Ok(())
}

#[tokio::test]
async fn file_picker_pages_without_first_jumping_to_the_bottom() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    for index in 0..200 {
        std::fs::write(dir.path().join(format!("{index:03}.txt")), "preview")?;
    }
    let mut picker = ui::file_picker(&app.editor, dir.path().to_owned());
    until(&mut picker, &mut app, |picker| {
        picker.results.matcher.snapshot().matched_item_count() == 200
    })
    .await?;
    let rows = picker.completion_height as u32;
    assert!(rows > 3, "test frame must show a page");
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let config = AppConfig::default();
    let mut jobs = Jobs::new();
    let mut press = |picker: &mut Picker<_, _>, app: &mut Application, key: &str| {
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: true,
        };
        picker.handle_event(&Event::Key(key.parse().unwrap()), &mut cx);
        draw(picker, app);
    };
    for _ in 0..3 {
        press(&mut picker, &mut app, "down");
    }
    let row = picker.results.cursor - picker.results.offset;
    assert_eq!(row, 3);
    press(&mut picker, &mut app, "pagedown");
    assert_eq!(picker.results.cursor, 3 + rows);
    assert_eq!(picker.results.cursor - picker.results.offset, row);
    Ok(())
}
