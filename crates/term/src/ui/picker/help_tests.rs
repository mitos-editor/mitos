use super::super::{test, *};
use crate::{application::Application, config::Config as AppConfig};

fn frame<T: Send + Sync, D: Send + Sync>(
    picker: &mut Picker<T, D>,
    app: &mut Application,
    config: &AppConfig,
) -> String {
    test::frame_with_config(picker, app, config)
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn set_hints(app: &mut Application, enabled: bool) {
    let mut config = (*app.editor.config()).clone();
    config.picker.show_keybindings = enabled;
    app.editor.config = Arc::new(arc_swap::access::Constant(config));
}

#[tokio::test]
async fn picker_specific_hints_follow_live_visibility_and_use_at_most_one_row() -> anyhow::Result<()>
{
    let mut app = test::application(false)?;
    let config = AppConfig::default();
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |item: &u8, _: &()| {
            item.to_string().into()
        })],
        0,
        0..40,
        (),
        |_, _, _| {},
    )
    .with_item_actions(
        |maps| &maps.quicklist_picker,
        |_, _, _| PickerCallbackResult::KeepOpen,
    );
    test::until(&mut picker, &mut app, |picker| {
        test::items(picker).count() == 40
    })
    .await?;
    assert!(frame(&mut picker, &mut app, &config).contains("<C-x> remove entry"));
    let visible_rows = picker.completion_height;
    set_hints(&mut app, false);
    assert!(!frame(&mut picker, &mut app, &config).contains("remove entry"));
    assert_eq!(picker.completion_height, visible_rows);
    assert_eq!(test::items(&picker).count(), 40);
    set_hints(&mut app, true);
    assert!(frame(&mut picker, &mut app, &config).contains("<C-x> remove entry"));
    assert_eq!(picker.completion_height, visible_rows);
    Ok(())
}

#[tokio::test]
async fn file_pickers_show_scroll_hints_without_losing_result_space() -> anyhow::Result<()> {
    let mut app = test::application(false)?;
    let config = AppConfig::default();
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("entry.txt"), "text")?;
    let mut picker = ui::file_picker(&app.editor, dir.path().to_owned());
    test::until(&mut picker, &mut app, |picker| {
        test::items(picker).count() == 1
    })
    .await?;
    let visible = frame(&mut picker, &mut app, &config);
    for generic in ["<ret> open", "split below", "split right", "quicklist"] {
        assert!(!visible.contains(generic), "unexpected {generic:?}");
    }
    assert!(visible.contains("<A-j> scroll preview down"));
    assert!(visible.contains("<A-k> scroll preview up"));
    let visible_rows = picker.completion_height;
    picker.toggle_preview();
    assert!(!frame(&mut picker, &mut app, &config).contains("scroll preview"));
    picker.toggle_preview();
    set_hints(&mut app, false);
    assert!(!frame(&mut picker, &mut app, &config).contains("scroll preview"));
    assert_eq!(picker.completion_height, visible_rows);
    Ok(())
}

#[tokio::test]
async fn item_hints_use_the_specific_picker_map_and_omit_unbound_actions() -> anyhow::Result<()> {
    let mut app = test::application(false)?;
    let source = "[keys.buffer-picker]\nF3 = 'close_buffer'\n[keys.quicklist-picker]\nC-x = 'unbound'\nF4 = 'remove_entry'".to_owned();
    let config = AppConfig::load(Ok(&source), Err(Default::default())).unwrap();
    let picker = || {
        Picker::new(
            app.editor.redraw().clone(),
            [Column::new("item", |item: &u8, _: &()| {
                item.to_string().into()
            })],
            0,
            [1],
            (),
            |_, _, _| {},
        )
    };
    let mut buffers = picker().with_item_actions(
        |maps| &maps.buffer_picker,
        |_, _, _| PickerCallbackResult::KeepOpen,
    );
    let mut quicklist = picker().with_item_actions(
        |maps| &maps.quicklist_picker,
        |_, _, _| PickerCallbackResult::KeepOpen,
    );
    let text = frame(&mut buffers, &mut app, &config);
    assert!(text.contains("<F3> close buffer"));
    assert!(!text.contains("remove entry"));
    let text = frame(&mut quicklist, &mut app, &config);
    assert!(text.contains("<F4> remove entry"));
    assert!(!text.contains("<C-x>"));
    assert!(!text.contains("close buffer"));
    Ok(())
}
