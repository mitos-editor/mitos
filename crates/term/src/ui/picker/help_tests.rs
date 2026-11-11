use super::super::{test, *};
use crate::{application::TestApplication, config::Config as AppConfig};

fn frame<T: Send + Sync, D: Send + Sync>(
    picker: &mut Picker<T, D>,
    app: &mut TestApplication,
    config: &AppConfig,
) -> String {
    test::frame_with_config(picker, app, config)
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn set_hints(app: &mut TestApplication, enabled: bool) {
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
    assert!(frame(&mut picker, &mut app, &config).contains("<C-x>d remove entry"));
    let visible_rows = picker.completion_height;
    set_hints(&mut app, false);
    assert!(!frame(&mut picker, &mut app, &config).contains("remove entry"));
    assert_eq!(picker.completion_height, visible_rows);
    assert_eq!(test::items(&picker).count(), 40);
    set_hints(&mut app, true);
    assert!(frame(&mut picker, &mut app, &config).contains("<C-x>d remove entry"));
    assert_eq!(picker.completion_height, visible_rows);
    Ok(())
}

#[tokio::test]
async fn file_pickers_show_direct_actions_and_menu_without_losing_result_space(
) -> anyhow::Result<()> {
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
    assert!(visible.contains("<C-s> split below"), "{visible}");
    assert!(visible.contains("<C-x> actions"));
    let visible_rows = picker.completion_height;
    picker.toggle_preview();
    let hidden_preview = frame(&mut picker, &mut app, &config);
    assert!(hidden_preview.contains("<C-s> split below"));
    assert!(hidden_preview.contains("<C-x> actions"));
    picker.toggle_preview();
    set_hints(&mut app, false);
    let hidden_hints = frame(&mut picker, &mut app, &config);
    assert!(!hidden_hints.contains("split below"));
    assert!(!hidden_hints.contains("<C-x> actions"));
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

#[tokio::test]
async fn picker_specializations_preserve_query_editing_and_show_available_prefixes(
) -> anyhow::Result<()> {
    type Map = fn(&ComponentKeymaps) -> &ui_core::keymap::Keymap<PickerAction>;
    let maps: [Map; 9] = [
        |m| &m.picker,
        |m| &m.buffer_picker,
        |m| &m.quicklist_picker,
        |m| &m.git_picker,
        |m| &m.branch_picker,
        |m| &m.worktree_picker,
        |m| &m.stash_picker,
        |m| &m.history_picker,
        |m| &m.file_explorer,
    ];
    let mut app = test::application(false)?;
    let config = AppConfig::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = crate::job::Jobs::new();
    for map in maps {
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [Column::new("item", |_: &u8, _: &()| "item".into())],
            0,
            [1],
            (),
            |_, _, _| {},
        )
        .with_item_actions(map, |_, _, _| PickerCallbackResult::KeepOpen);
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: true,
        };
        for key in ["a", "b", "c", "home", "C-d"] {
            picker.handle_event(&Event::Key(key.parse()?), &mut cx);
        }
        assert_eq!(picker.prompt.line(), "bc");
        for key in ["end", "C-u"] {
            picker.handle_event(&Event::Key(key.parse()?), &mut cx);
        }
        assert_eq!(picker.prompt.line(), "");
        picker.handle_event(&Event::Key("C-x".parse()?), &mut cx);
        let contexts = picker.keymap_contexts(&cx);
        assert_eq!(contexts[0].pending, ["C-x".parse()?]);
        let area = Rect::new(0, 0, 100, 24);
        let mut surface = Surface::empty(area);
        picker.update_and_render(area, &mut surface, &mut cx);
        let screen: String = surface.content.iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("Actions"), "{screen}");
        assert!(!screen.contains("Open in a vertical split"));
        picker.handle_event(&Event::Key("C-c".parse()?), &mut cx);
        assert!(!picker.keys.is_pending());
        picker.handle_event(&Event::Key("x".parse()?), &mut cx);
        assert_eq!(picker.prompt.line(), "x");
    }
    Ok(())
}

#[tokio::test]
async fn git_shortcuts_and_menu_routes_operate_on_the_same_selected_item() -> anyhow::Result<()> {
    use std::{cell::RefCell, rc::Rc};
    let mut app = test::application(false)?;
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = actions.clone();
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |_: &u8, _: &()| "file".into())],
        0,
        [1],
        (),
        |_, _, _| {},
    )
    .with_item_actions(
        |maps| &maps.git_picker,
        move |_, item, action| {
            recorded.borrow_mut().push((*item.unwrap(), action));
            PickerCallbackResult::KeepOpen
        },
    );
    test::until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
    let config = AppConfig::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = crate::job::Jobs::new();
    let mut cx = Context {
        config: crate::config::Context {
            current: &config,
            updates: &updates,
        },
        editor: &mut app.editor,
        interaction: &mut app.interaction,
        jobs: &mut jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    for key in ["A-s", "A-u", "C-x", "a", "s", "C-x", "a", "u"] {
        picker.handle_event(&Event::Key(key.parse()?), &mut cx);
    }
    assert_eq!(
        *actions.borrow(),
        [
            (1, PickerItemAction::Stage),
            (1, PickerItemAction::Unstage),
            (1, PickerItemAction::Stage),
            (1, PickerItemAction::Unstage)
        ]
    );
    Ok(())
}
