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
async fn file_picker_hints_follow_live_visibility_and_restore_result_space() -> anyhow::Result<()> {
    let mut app = test::application(false)?;
    let config = AppConfig::default();
    let dir = tempfile::tempdir()?;
    for index in 0..40 {
        std::fs::write(dir.path().join(format!("{index:02}.txt")), "text")?;
    }
    let mut picker = ui::file_picker(&app.editor, dir.path().to_owned());
    test::until(&mut picker, &mut app, |picker| {
        test::items(picker).count() == 40
    })
    .await?;
    let visible = frame(&mut picker, &mut app, &config);
    for hint in [
        "<ret> open",
        "<C-s> split below",
        "<C-v> split right",
        "<C-t> preview",
        "<C-q> quicklist",
    ] {
        assert!(visible.contains(hint), "missing {hint:?}");
    }
    assert!(!visible.contains("replace"));
    let visible_rows = picker.completion_height;
    set_hints(&mut app, false);
    let hidden = frame(&mut picker, &mut app, &config);
    assert!(!hidden.contains("<ret> open"));
    assert!(!hidden.contains("split below"));
    assert!(picker.completion_height > visible_rows);
    assert_eq!(test::items(&picker).count(), 40);
    set_hints(&mut app, true);
    assert!(frame(&mut picker, &mut app, &config).contains("<ret> open"));
    Ok(())
}

#[tokio::test]
async fn generic_picker_hints_only_advertise_supported_actions() -> anyhow::Result<()> {
    let mut app = test::application(false)?;
    let config = AppConfig::default();
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |item: &u8, _: &()| {
            item.to_string().into()
        })],
        0,
        [1, 2],
        (),
        |_, _, _| {},
    );
    let text = frame(&mut picker, &mut app, &config);
    assert!(text.contains("<ret> select"));
    assert!(text.contains("close"));
    for unavailable in [
        "split",
        "preview",
        "quicklist",
        "replace",
        "remove entry",
        "close buffer",
    ] {
        assert!(!text.contains(unavailable), "unexpected {unavailable:?}");
    }
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
