use super::*;
use crate::keymap::component::ComponentKeymaps;
use crate::keymap::{self, merge_keys, KeyTrie};
use ui_core::terminal::KittyKeyboardProtocolConfig;
use view::document::Mode;

impl Config {
    fn load_test(config: &str) -> Config {
        Config::load(Ok(&config.to_owned()), Err(ConfigLoadError::default())).unwrap()
    }

    fn load_test_result(config: &str) -> Result<Config, ConfigLoadError> {
        Config::load(Ok(&config.to_owned()), Err(ConfigLoadError::default()))
    }
}

#[test]
fn parsing_keymaps_config_file() {
    use crate::keymap;
    use editor_core::hashmap;
    use view::document::Mode;

    let sample_keymaps = r#"
            [keys.insert]
            y = "move_line_down"
            S-C-a = "delete_selection"

            [keys.normal]
            A-F12 = "move_next_word_end"
        "#;

    let mut keys = keymap::default();
    merge_keys(
        &mut keys,
        hashmap! {
            Mode::Insert => keymap!({ "Insert mode"
                "y" => move_line_down,
                "S-C-a" => delete_selection,
            }),
            Mode::Normal => keymap!({ "Normal mode"
                "A-F12" => move_next_word_end,
            }),
        },
    );

    assert_eq!(
        Config::load_test(sample_keymaps),
        Config {
            keys,
            ..Default::default()
        }
    );
}

#[test]
fn labeled_editor_bindings_preserve_commands_and_descriptions() {
    use crate::keymap::MappableCommand;
    use ui_core::input::KeyEvent;

    let config = Config::load_test(
        r#"
            [keys.normal.space.f]
            label = "File"
            s = { label = "Save", command = ":w" }
            c = { command = ":open ~/.config/mitos/config.toml", label = "Edit Config" }
            m = { label = "Delete word", command = "@wd" }
            h = { label = "Split left", command = ["vsplit", "normal_mode", "swap_view_left"] }
            j = ["hsplit", "normal_mode", "swap_view_down"]
            k = { command = "move_char_left" }
            n = { label = "One command", command = ["move_char_left"] }
            e = { label = "Empty", command = [] }
            [keys.normal.space.f.x]
            label = "Extra"
            r = { label = "Reload", command = ":reload" }
            [keys.insert]
            C-s = { label = "Save", command = ":write" }
            [keys.select]
            C-s = { label = "Save", command = ":write" }
        "#,
    );
    let path: Vec<KeyEvent> = ["space", "f"].map(|key| key.parse().unwrap()).into();
    let menu = config.keys[&Mode::Normal]
        .search(&path)
        .unwrap()
        .node()
        .unwrap();
    let binding = |key: &str| &menu[&key.parse::<KeyEvent>().unwrap()];
    let KeyTrie::MappableCommand(save) = binding("s") else {
        panic!("expected typable command");
    };
    assert_eq!(save.name(), "write");
    assert_eq!(save.doc(), "Save");
    let KeyTrie::MappableCommand(MappableCommand::Typable { name, args, doc }) = binding("c")
    else {
        panic!("expected open command");
    };
    assert_eq!(name, "open");
    assert_eq!(args, "~/.config/mitos/config.toml");
    assert_eq!(doc, "Edit Config");
    let KeyTrie::MappableCommand(mac) = binding("m") else {
        panic!("expected macro");
    };
    assert_eq!(mac.name(), "@wd");
    assert_eq!(mac.to_string(), "@wd");
    assert_eq!(mac.doc(), "Delete word");
    let MappableCommand::Macro { keys, .. } = mac else {
        panic!("expected macro keys");
    };
    assert_eq!(*keys, ui_core::input::parse_macro("wd").unwrap());
    assert_eq!(
        binding("k"),
        &KeyTrie::MappableCommand(MappableCommand::move_char_left)
    );
    assert_eq!(
        binding("n"),
        &KeyTrie::Sequence("One command".into(), vec![MappableCommand::move_char_left])
    );
    assert_eq!(binding("e"), &KeyTrie::Sequence("Empty".into(), vec![]));
    let info = menu.infobox();
    assert_eq!(info.title, "File");
    for description in [
        "Save",
        "Edit Config",
        "Delete word",
        "Split left",
        KeyTrie::DEFAULT_SEQUENCE_LABEL,
        "Extra",
    ] {
        assert!(info.text.contains(description), "{}", info.text);
    }
    assert_eq!(binding("x").node().unwrap().infobox().title, "Extra");
    assert!(binding("x")
        .node()
        .unwrap()
        .infobox()
        .text
        .contains("Reload"));
    assert_eq!(
        config.keys[&Mode::Normal].reverse_map()["write"],
        vec![vec![
            "space".parse().unwrap(),
            "f".parse().unwrap(),
            "s".parse().unwrap()
        ]]
    );
    for mode in [Mode::Insert, Mode::Select] {
        let KeyTrie::MappableCommand(cmd) = config.keys[&mode]
            .search(&["C-s".parse().unwrap()])
            .unwrap()
        else {
            panic!("expected a labeled binding in {mode:?}");
        };
        assert_eq!(cmd.doc(), "Save");
    }
}

#[test]
fn menu_labels_merge_defaults_global_and_workspace_recursively() {
    use ui_core::input::KeyEvent;

    let global = r#"
            [keys.normal]
            label = "Custom normal"
            [keys.normal.g]
            label = "Navigate"
            x = { label = "Global", command = ":echo global" }
            [keys.normal.g.z]
            label = "Nested"
            e = ":echo nested"
        "#
    .to_owned();
    for (label, expected) in [
        ("", "Navigate"),
        ("label = ''", "Navigate"),
        ("label = 'Project navigation'", "Project navigation"),
    ] {
        let local = format!("[keys.normal.g]\n{label}\nx = {{ label = 'Local', command = ':echo local' }}\n[keys.normal.g.z]\nr = ':reload'");
        let config = Config::load(Ok(&global), Ok(local)).unwrap();
        let root = config.keys[&Mode::Normal].node().unwrap();
        assert_eq!(root.infobox().title, "Custom normal");
        let goto = root[&"g".parse::<KeyEvent>().unwrap()].node().unwrap();
        assert_eq!(goto.infobox().title, expected);
        assert_eq!(
            goto[&"g".parse::<KeyEvent>().unwrap()],
            KeyTrie::MappableCommand(crate::commands::MappableCommand::goto_file_start)
        );
        assert!(goto.infobox().text.contains("Local"));
        let nested = goto[&"z".parse::<KeyEvent>().unwrap()].node().unwrap();
        assert_eq!(nested.infobox().title, "Nested");
        assert!(nested.contains_key(&"e".parse::<KeyEvent>().unwrap()));
        assert!(nested.contains_key(&"r".parse::<KeyEvent>().unwrap()));
    }
}

#[test]
fn empty_binding_labels_keep_default_descriptions() {
    use ui_core::input::KeyEvent;

    let config = Config::load_test(
        r#"
            [keys.normal]
            a = { label = "", command = ":buffer-close" }
            b = ":buffer-close"
            c = { label = "", command = "@wd" }
            d = "@wd"
            e = { label = "", command = ["select_all"] }
            f = ["select_all"]
            h = { label = "", command = "move_char_left" }
        "#,
    );
    let root = config.keys[&Mode::Normal].node().unwrap();
    for (left, right) in [("a", "b"), ("c", "d"), ("e", "f")] {
        let description = |key: &str| match &root[&key.parse::<KeyEvent>().unwrap()] {
            KeyTrie::MappableCommand(cmd) => cmd.doc(),
            KeyTrie::Sequence(label, _) => label,
            KeyTrie::Node(_) => panic!("expected a command or sequence"),
        };
        assert_eq!(description(left), description(right));
    }
}

#[test]
fn labeled_bindings_reject_invalid_commands_and_shapes() {
    for binding in [
        "{ label = 'Static', command = 'move_char_left' }",
        "{ label = 'Unknown', command = ':missing-command' }",
        "{ label = 'Unknown', command = 'missing_command' }",
        "{ label = 'Mixed', command = ':write', x = 'no_op' }",
        "{ label = 'Macro sequence', command = ['@wd', ':write'] }",
        "{ command = ['@wd'] }",
        "{ command = 1 }",
        "{ command = [':write', 1] }",
        "{ command = { command = ':write' } }",
        "{ label = 1, command = ':write' }",
        "{ lable = 'Misspelled', command = ':write' }",
        "{ label = 'Duplicates', C-S-a = ':write', S-C-a = ':write' }",
        "['@wd', ':write']",
    ] {
        assert!(
            Config::load_test_result(&format!("[keys.normal]\nx = {binding}")).is_err(),
            "{binding}"
        );
    }
    // Mode roots must remain keymaps, even with the command table syntax.
    assert!(Config::load_test_result("[keys.normal]\nlabel = 'Save'\ncommand = ':write'").is_err());
}

#[test]
fn keys_resolve_to_correct_defaults() {
    // From serde default
    let default_keys = Config::load_test("").keys;
    assert_eq!(default_keys, keymap::default());

    // From the Default trait
    let default_keys = Config::default().keys;
    assert_eq!(default_keys, keymap::default());
}

#[test]
fn component_keys_merge_defaults_global_and_workspace_recursively() {
    use crate::keymap::component::PickerAction;
    use ui_core::keymap::{KeymapState, Lookup};

    let global =
        "[keys.picker]\nC-j = 'move_down'\n[ keys.picker.g ]\nj = 'move_down'\nk = 'move_up'"
            .to_owned();
    let local = "[keys.picker]\nC-j = 'move_up'\nC-n = 'unbound'\n[ keys.picker.g ]\nj = ['move_start', 'move_down']".to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();
    let mut state = KeymapState::default();
    let keys = &config.component_keys.picker;
    let mut lookup = |key: &str| state.lookup(keys, key.parse().unwrap());
    assert_eq!(lookup("C-j"), Lookup::Actions(&[PickerAction::MoveUp]));
    assert_eq!(lookup("C-n"), Lookup::Unbound);
    assert_eq!(lookup("tab"), Lookup::Actions(&[PickerAction::MoveDown]));
    assert_eq!(lookup("g"), Lookup::Pending);
    assert_eq!(
        lookup("j"),
        Lookup::Actions(&[PickerAction::MoveStart, PickerAction::MoveDown])
    );
    assert_eq!(lookup("g"), Lookup::Pending);
    assert_eq!(lookup("k"), Lookup::Actions(&[PickerAction::MoveUp]));
    assert_eq!(config.keys, keymap::default());
    assert_eq!(
        Config::load_test("").component_keys,
        ComponentKeymaps::default()
    );
}

#[test]
fn workspace_keymap_replacements_do_not_restore_removed_defaults() {
    use crate::keymap::component::PickerAction;
    use ui_core::keymap::{KeymapState, Lookup};

    let global =
        "[keys.normal]\ng = 'no_op'\n[keys.picker]\ntab = { j = 'move_down', k = 'move_up' }"
            .to_owned();
    let local =
        "[keys.normal]\ng = { x = 'move_char_left' }\n[keys.picker]\ntab = 'no_op'".to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();
    // Replacing a prefix, then restoring it, must not resurrect its defaults.
    let keymap::KeyTrie::Node(prefix) = &config.keys[&Mode::Normal].node().unwrap()
        [&"g".parse::<ui_core::input::KeyEvent>().unwrap()]
    else {
        panic!("expected the workspace prefix");
    };
    assert_eq!(prefix.len(), 1);
    assert!(prefix.contains_key(&"x".parse::<ui_core::input::KeyEvent>().unwrap()));
    let mut state = KeymapState::default();
    assert_eq!(
        state.lookup(&config.component_keys.picker, "tab".parse().unwrap()),
        Lookup::Actions(&[])
    );
    assert_eq!(
        state.lookup(&config.component_keys.picker, "j".parse().unwrap()),
        Lookup::Unbound
    );
    assert_eq!(
        state.lookup(
            &config.component_keys.buffer_picker,
            "down".parse().unwrap()
        ),
        Lookup::Actions(&[PickerAction::MoveDown])
    );
}

#[test]
fn component_keys_reject_invalid_actions_shapes_and_contexts() {
    for config in [
        "[keys.unknown]\nj = 'move_down'",
        "[keys.picker]\nj = 'scroll_down'",
        "[keys.picker]\nC-x = 'close_buffer'",
        "[keys.picker]\nA-s = 'stage'",
        "[keys.buffer-picker]\nA-s = 'stage'",
        "[keys.git-picker]\nA-s = 'apply_stash'",
        "[keys.stash-picker]\nA-s = 'stage'",
        "[keys.menu]\nC-x = 'close_buffer'",
        "[keys.buffer-picker]\nC-x = 'scroll_down'",
        "[keys.picker]\nC-x = 'remove_entry'",
        "[keys.buffer-picker]\nC-x = 'remove_entry'",
        "[keys.quicklist-picker]\nC-x = 'close_buffer'",
        "[keys.prompt]\nj = 'move_down'",
        "[keys.menu]\nj = ':write'",
        "[keys.popup]\nj = '@abc'",
        "[keys.hover]\nj = 'missing_action'",
        "[keys.signature-help]\nj = 1",
        "[keys.picker]\nj = ['move_down', 'unbound']",
        "[keys.picker]\nnot-a-key = 'move_down'",
        "[keys]\nnormal = 'no_op'",
        // Both notations normalize to the same key event.
        "[keys.picker]\nC-S-a = 'move_down'\nS-C-a = 'move_up'",
    ] {
        assert!(Config::load_test_result(config).is_err(), "{config}");
    }
}

#[test]
fn buffer_picker_keys_inherit_shared_bindings_with_specific_overrides() {
    use crate::keymap::component::{PickerAction as P, PickerItemAction};
    use ui_core::keymap::{KeymapState, Lookup};

    let global = r#"
            [keys.picker]
            C-j = 'move_down'
            g = { j = 'move_up' }
            [keys.buffer-picker]
            C-j = 'move_up'
            C-x = ['move_down', 'close_buffer']
            g = { x = 'close_buffer' }
        "#
    .to_owned();
    let local = r#"
            [keys.picker]
            C-j = 'move_start'
            g = { j = 'move_end' }
            [keys.buffer-picker]
            C-n = 'unbound'
            C-p = 'no_op'
        "#
    .to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();
    let mut state = KeymapState::default();
    let keys = &config.component_keys.buffer_picker;
    let mut lookup = |key: &str| state.lookup(keys, key.parse().unwrap());
    assert_eq!(lookup("C-j"), Lookup::Actions(&[P::MoveUp]));
    assert_eq!(
        lookup("C-x"),
        Lookup::Actions(&[P::MoveDown, P::Item(PickerItemAction::CloseBuffer)])
    );
    assert_eq!(lookup("C-n"), Lookup::Unbound);
    assert_eq!(lookup("C-p"), Lookup::Actions(&[]));
    assert_eq!(lookup("tab"), Lookup::Actions(&[P::MoveDown]));
    assert_eq!(lookup("g"), Lookup::Pending);
    assert_eq!(lookup("j"), Lookup::Actions(&[P::MoveEnd]));
    assert_eq!(lookup("g"), Lookup::Pending);
    assert_eq!(
        lookup("x"),
        Lookup::Actions(&[P::Item(PickerItemAction::CloseBuffer)])
    );
    assert_eq!(
        state.lookup(&config.component_keys.picker, "C-j".parse().unwrap()),
        Lookup::Actions(&[P::MoveStart])
    );
    assert_eq!(
        state.lookup(&config.component_keys.picker, "C-x".parse().unwrap()),
        Lookup::Unbound
    );
    let defaults = Config::default().component_keys;
    assert_eq!(defaults.buffer_picker, defaults.picker);
}

#[test]
fn quicklist_picker_keys_inherit_shared_bindings_with_specific_overrides() {
    use crate::keymap::component::{PickerAction as P, PickerItemAction};
    use ui_core::keymap::{KeymapState, Lookup};

    let global = r#"
        [keys.picker]
        C-j = 'move_down'
        [keys.quicklist-picker]
        C-j = 'move_up'
        C-x = 'unbound'
        C-r = ['move_down', 'remove_entry']
    "#
    .to_owned();
    let local =
        "[keys.picker]\nC-j = 'move_start'\n[keys.quicklist-picker]\nC-n = 'no_op'".to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();
    let mut state = KeymapState::default();
    let keys = &config.component_keys.quicklist_picker;
    assert_eq!(
        state.lookup(keys, "C-j".parse().unwrap()),
        Lookup::Actions(&[P::MoveUp])
    );
    assert_eq!(state.lookup(keys, "C-x".parse().unwrap()), Lookup::Unbound);
    assert_eq!(
        state.lookup(keys, "C-n".parse().unwrap()),
        Lookup::Actions(&[])
    );
    assert_eq!(
        state.lookup(keys, "tab".parse().unwrap()),
        Lookup::Actions(&[P::MoveDown])
    );
    assert_eq!(
        state.lookup(keys, "C-r".parse().unwrap()),
        Lookup::Actions(&[P::MoveDown, P::Item(PickerItemAction::RemoveQuicklistEntry)])
    );
    assert_eq!(
        state.lookup(&config.component_keys.picker, "C-j".parse().unwrap()),
        Lookup::Actions(&[P::MoveStart])
    );
    assert_eq!(
        state.lookup(&config.component_keys.picker, "C-x".parse().unwrap()),
        Lookup::Unbound
    );
    assert_eq!(
        state.lookup(
            &Config::default().component_keys.quicklist_picker,
            "C-x".parse().unwrap()
        ),
        Lookup::Actions(&[P::Item(PickerItemAction::RemoveQuicklistEntry)])
    );
}

#[test]
fn terminal_settings_keep_editor_table_and_merge_precedence() {
    assert_eq!(Config::load_test("").terminal, TerminalConfig::default());
    let global = "[editor]\ntrue-color = true\nundercurl = true\nkitty-keyboard-protocol = 'disabled'\nscrolloff = 7".to_owned();
    let local = "[editor]\ntrue-color = false\nkitty-keyboard-protocol = 'enabled'\n[editor.auto-save.after-delay]\nenable = true\ntimeout = 2500".to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();

    assert_eq!(
        config.terminal,
        TerminalConfig {
            true_color: false,
            undercurl: true,
            keyboard: KeyboardConfig {
                kitty_protocol: KittyKeyboardProtocolConfig::Enabled
            },
            ..TerminalConfig::default()
        }
    );
    assert_eq!(config.editor.scroll.padding, 7);
    assert!(config.editor.files.auto_save.after_delay.enable);
    assert_eq!(config.editor.files.auto_save.after_delay.timeout, 2500);

    // The shared editor schema no longer contains terminal capabilities.
    let shared = serde_json::to_value(&config.editor).unwrap();
    assert!(shared.get("true-color").is_none());
    assert!(shared.get("undercurl").is_none());
    assert!(shared.get("kitty-keyboard-protocol").is_none());
}

#[test]
fn composed_settings_retain_strict_validation() {
    for settings in [
        "true-color = 'yes'",
        "undercurl = 1",
        "kitty-keyboard-protocol = 'sometimes'",
        "undercurls = true",
        "scrolloff = 'seven'",
        "auto-save = { after-delay = { unknown = true } }",
    ] {
        assert!(
            Config::load_test_result(&format!("[editor]\n{settings}")).is_err(),
            "{settings}"
        );
    }
}

#[test]
fn composed_settings_round_trip_for_runtime_options() {
    let config = Config::load_test("[editor]\ntrue-color = true\nundercurl = true\nkitty-keyboard-protocol = 'disabled'\nidle-timeout = 123\n[editor.cursor-shape]\ninsert = 'bar'");
    let settings = config.editor_settings();
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(value["terminal"]["true-color"], true);
    assert_eq!(value["idle-timeout"], 123);
    assert_eq!(value["cursor"]["shape"]["insert"], "bar");
    assert_eq!(
        serde_json::from_value::<EditorSettings>(value).unwrap(),
        settings
    );
}

#[test]
fn inline_blame_settings_round_trip_with_grouped_editor_settings() {
    use editor_config::InlineBlameShow;

    let config = Config::load_test(
            "[editor.inline-blame]\nshow = 'all-lines'\nformat = '{title}'\n[editor.scroll]\npadding = 7",
        );
    let settings = config.editor_settings();
    assert_eq!(settings.editor.inline_blame.show, InlineBlameShow::AllLines);
    assert_eq!(settings.editor.inline_blame.format, "{title}");
    assert_eq!(settings.editor.scroll.padding, 7);
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(value["inline-blame"]["show"], "all-lines");
    assert_eq!(
        serde_json::from_value::<EditorSettings>(value).unwrap(),
        settings
    );
    assert!(Config::load_test_result("[editor.inline-blame]\nshow = 'sometimes'").is_err());
    assert!(Config::load_test_result("[editor.inline-blame]\nauto-fetch = true").is_err());
}

#[test]
fn icons_are_controlled_by_one_editor_option() {
    assert!(!Config::load_test("").editor.icons);
    assert!(Config::load_test("[editor]\nicons = true").editor.icons);
}

#[test]
fn breadcrumbs_are_disabled_by_default_and_configurable() {
    use editor_config::BreadcrumbPathOptions;

    let default = Config::load_test("").editor.breadcrumb;
    assert!(!default.enable);
    assert_eq!(default.path, BreadcrumbPathOptions::Full);

    let configured = Config::load_test("[editor.breadcrumb]\nenable = true\npath = \"file\"")
        .editor
        .breadcrumb;
    assert!(configured.enable);
    assert_eq!(configured.path, BreadcrumbPathOptions::File);
}

#[test]
fn welcome_screen_is_enabled_by_default_and_can_be_disabled() {
    assert!(Config::load_test("").editor.welcome_screen);
    assert!(
        !Config::load_test("[editor]\nwelcome-screen = false")
            .editor
            .welcome_screen
    );
}

#[test]
fn border_style_is_an_editor_setting_independent_of_the_theme() {
    let config = Config::load_test("theme = 'default'\n[editor]\nborder-style = 'rounded'");
    assert_eq!(
        config.editor.border_style,
        editor_config::BorderStyle::Rounded
    );
}

#[test]
fn picker_layout_is_shared_by_all_pickers() {
    assert_eq!(
        Config::load_test("").editor.picker.layout,
        editor_config::PickerLayout::Auto
    );
    let config = Config::load_test("[editor.picker]\nlayout = 'centered'");
    assert_eq!(
        config.editor.picker.layout,
        editor_config::PickerLayout::Centered
    );
}

#[test]
fn popup_border_is_not_configurable() {
    let config = "[editor]\npopup-border = \"none\"".to_owned();
    let error = Config::load(Ok(&config), Err(ConfigLoadError::default())).unwrap_err();
    assert!(error.to_string().contains("unknown field `popup-border`"));
}

#[test]
fn deserializes_custom_commands() {
    let config = Config::load_test(
        r#"
[commands]
":wq" = [":write", ":quit"]
":w" = ":write!"
"0" = ":goto 1"

[commands.":wcd!"]
commands = [":write! %arg{0}", ":cd %sh{ %arg{0} | path dirname }"]
desc = "Force save, then change directory"
accepts = "<path>"
completer = ":write"
"#,
    );

    assert!(config.editor.commands.get("wq").is_some());
    assert!(config.editor.commands.get("0").unwrap().hidden);
    assert_eq!(
        config
            .editor
            .commands
            .get("wcd!")
            .unwrap()
            .completer
            .as_deref(),
        Some("write")
    );
}

#[test]
fn local_custom_commands_override_global_commands() {
    let global = "[commands]\n':save' = ':write'".to_owned();
    let local = "[commands]\n':save' = ':write!'\n':quit' = ':quit'".to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();

    assert_eq!(
        config.editor.commands.get("save").unwrap().commands,
        [":write!"]
    );
    assert!(config.editor.commands.get("quit").is_some());
}

#[test]
fn rejects_macros_in_command_sequences() {
    let error =
        Config::load_test_result("[commands]\n':fail' = { commands = ['@100xd', ':write'] }")
            .unwrap_err();
    assert!(error
        .to_string()
        .contains("macro keybindings may not be used in command sequences"));
}

#[test]
fn clipboard_settings_roundtrip_and_local_override_preserve_schema() {
    use ::clipboard::ClipboardProvider;
    let global = "[editor]\nclipboard-provider = 'termcode'".to_owned();
    let local = "[editor]\nclipboard-provider = 'none'".to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();
    assert_eq!(config.editor.clipboard.provider, ClipboardProvider::None);
    let custom = Config::load_test(
        r#"
            [editor.clipboard-provider.custom]
            yank = { command = "read-clipboard", args = ["--text"] }
            paste = { command = "write-clipboard" }
            paste-primary = { command = "write-primary" }
        "#,
    );
    let settings = custom.editor_settings();
    let serialized = serde_json::to_value(&settings).unwrap();
    assert_eq!(
        serialized["clipboard"]["provider"]["custom"]["yank"]["command"],
        "read-clipboard"
    );
    assert_eq!(
        serialized["clipboard"]["provider"]["custom"]["paste"]["args"],
        serde_json::json!([])
    );
    assert_eq!(
        serde_json::from_value::<EditorSettings>(serialized).unwrap(),
        settings
    );
    assert!(Config::load_test_result("[editor]\nclipboard-provider = 'unknown'").is_err());
}
#[test]
fn reorganized_defaults_keep_existing_behavior() {
    let config = Config::load_test("");
    let editor = config.editor;
    assert!(editor.lsp.enable);
    assert!(editor.lsp.messages.enable);
    assert!(!editor.lsp.progress.enable);
    assert!(editor.lsp.signature_help.auto_trigger);
    assert!(editor.lsp.signature_help.show_documentation);
    assert!(!editor.lsp.inlay_hints.enable);
    assert_eq!(editor.lsp.inlay_hints.length_limit, None);
    assert!(editor.lsp.document_highlight.auto_trigger);
    assert!(editor.lsp.color_swatches.enable);
    assert!(editor.lsp.code_lenses.enable);
    assert!(editor.lsp.completion.snippets);
    assert!(editor.lsp.references.include_declaration);
    assert!(editor.completion.auto_trigger);
    assert_eq!(editor.completion.trigger_delay.as_millis(), 250);
    assert_eq!(editor.inline_completion.trigger_delay.as_millis(), 150);
    assert!(editor.format.on_save);
    assert!(editor.files.save.atomic);
    assert!(editor.files.save.insert_final_newline);
    assert!(!editor.files.save.trim_final_newlines);
    assert!(!editor.files.save.trim_trailing_whitespace);
    assert!(!editor.statusline.mode.color);
    assert!(!editor.rainbow_brackets.enable);
}

#[test]
fn canonical_features_preserve_legacy_behavior_and_runtime_shape() {
    let old = Config::load_test(
        r#"
            [editor]
            mouse = false
            middle-click-paste = false
            bufferline = "always"
            color-modes = true
            cursorline = true
            auto-pairs = { '(' = ')' }
            auto-save = true
            auto-format = false
            rainbow-brackets = true
            completion-trigger-len = 1
            trim-final-newlines = true
            [editor.lsp]
            display-progress-messages = true
            display-inlay-hints = true
            inlay-hints-length-limit = 30
            [editor.cursor-shape]
            insert = "bar"
        "#,
    );
    let canonical = Config::load_test(
        r#"
            [editor.mouse]
            enable = false
            middle-click-paste = false
            [editor.bufferline]
            show = "always"
            [editor.statusline.mode]
            color = true
            [editor.cursor]
            highlight-line = true
            [editor.cursor.shape]
            insert = "bar"
            [editor.auto-pairs]
            pairs = { '(' = ')' }
            [editor.files.auto-save]
            focus-lost = true
            [editor.format]
            on-save = false
            [editor.rainbow-brackets]
            enable = true
            [editor.completion]
            trigger-length = 1
            [editor.files.save]
            trim-final-newlines = true
            [editor.lsp.progress]
            enable = true
            [editor.lsp.inlay-hints]
            enable = true
            length-limit = 30
        "#,
    );
    assert_eq!(old, canonical);
    let value = serde_json::to_value(canonical.editor_settings()).unwrap();
    assert_eq!(value["lsp"]["progress"]["enable"], true);
    assert_eq!(value["completion"]["trigger-length"], 1);
    assert!(value["lsp"].get("display-progress-messages").is_none());
    assert!(value.get("completion-trigger-len").is_none());
}

#[test]
fn mixed_spellings_respect_precedence_deep_inheritance_and_array_replacement() {
    for (global, local) in [
        (
            "[editor.lsp]\ndisplay-messages = false",
            "[editor.lsp.messages]\nenable = true",
        ),
        (
            "[editor.lsp.messages]\nenable = false",
            "[editor.lsp]\ndisplay-messages = true",
        ),
    ] {
        let config = Config::load(Ok(&global.to_owned()), Ok(local.to_owned())).unwrap();
        assert!(config.editor.lsp.messages.enable);
    }
    let global = r#"
            [editor]
            rulers = [80, 120]
            [editor.files.auto-save.after-delay]
            enable = true
            timeout = 1700
            [editor.files.reload.poll]
            enable = false
            interval = 2000
            [terminal]
            true-color = true
            [terminal.launcher]
            command = "tmux"
            args = ["split-window"]
        "#
    .to_owned();
    let local = r#"
            [editor]
            rulers = []
            [editor.auto-save.after-delay]
            timeout = 900
            [editor.files.reload.poll]
            interval = 4000
        "#
    .to_owned();
    let config = Config::load(Ok(&global), Ok(local)).unwrap();
    assert!(config.editor.files.auto_save.after_delay.enable);
    assert_eq!(config.editor.files.auto_save.after_delay.timeout, 900);
    assert!(!config.editor.files.reload.poll.enable);
    assert_eq!(config.editor.files.reload.poll.interval, 4000);
    assert!(config.editor.rulers.columns.is_empty());
    assert!(config.terminal.true_color);
    assert_eq!(config.terminal.launcher.unwrap().command, "tmux");
}

#[test]
fn duplicate_aliases_and_unknown_nested_settings_are_rejected() {
    for source in [
        "[editor.lsp]\ndisplay-messages = true\nmessages.enable = false",
        "[editor]\nauto-format = true\nformat.on-save = false",
        "[editor]\ntrue-color = true\n[terminal]\ntrue-color = false",
        "[editor.lsp.messages]\nenabled = true",
        "[editor.files.reload.poll]\nintervall = 1000",
        "[editor.completion.path]\nunknown = false",
        "[editor.auto-pairs]\nenabel = false",
        "[terminal.keyboard]\nkitty-protcol = 'auto'",
    ] {
        assert!(Config::load_test_result(source).is_err(), "{source}");
    }
    // Disjoint leaves in legacy and canonical containers can coexist.
    let config = Config::load_test(
        "[editor.word-completion]\nenable = false\n[editor.completion.word]\ntrigger-length = 3",
    );
    assert!(!config.editor.completion.word.enable);
    assert_eq!(config.editor.completion.word.trigger_length.get(), 3);
    let config = Config::load_test(
        "[editor]\nword-completion = {}\n[editor.completion.word]\nenable = false",
    );
    assert!(!config.editor.completion.word.enable);
}

#[test]
fn rulers_normalize_legacy_arrays_before_layering() {
    for (user, project, expected) in [
        (
            "[editor]\nrulers = [80, 120]",
            "[editor.rulers]\nstyle = 'glyph'\ncharacter = '|'",
            vec![80, 120],
        ),
        (
            "[editor.rulers]\ncolumns = [80, 120]\nstyle = 'glyph'\ncharacter = '|'",
            "[editor]\nrulers = []",
            vec![],
        ),
        (
            "[editor]\nrulers = [80]",
            "[editor.rulers]\ncolumns = [40]\nstyle = 'glyph'\ncharacter = '|'",
            vec![40],
        ),
    ] {
        let config = Config::load(Ok(&user.to_owned()), Ok(project.to_owned())).unwrap();
        assert_eq!(config.editor.rulers.columns, expected);
        assert_eq!(config.editor.rulers.character, '|');
        assert_eq!(config.editor.rulers.style, editor_config::RulerStyle::Glyph);
        let serialized = serde_json::to_value(config.editor_settings()).unwrap();
        assert_eq!(serialized["rulers"]["character"], "|");
        assert_eq!(serialized["rulers"]["style"], "glyph");
    }
    assert!(Config::load_test_result("[editor.rulers]\nchar = '|'").is_err());
    assert!(Config::load_test_result("[editor]\nrulers = [80]\nrulers.columns = [40]").is_err());
}

#[test]
fn git_picker_maps_inherit_shared_keys_and_keep_context_actions_scoped() {
    use crate::keymap::component::{PickerAction as P, PickerItemAction as I};
    use ui_core::keymap::{KeymapState, Lookup};
    let config = Config::load(
        Ok(&"[keys.git-picker]\nC-j = 'stage'\nA-z = 'unbound'\n[keys.stash-picker]\nC-j = 'apply_stash_with_index'".to_owned()),
        Ok("[keys.picker]\nC-j = 'move_up'\nC-k = 'move_down'".to_owned()),
    ).unwrap();
    let maps = &config.component_keys;
    let mut state = KeymapState::default();
    assert_eq!(
        state.lookup(&maps.git_picker, "C-j".parse().unwrap()),
        Lookup::Actions(&[P::Item(I::Stage)])
    );
    assert_eq!(
        state.lookup(&maps.stash_picker, "C-j".parse().unwrap()),
        Lookup::Actions(&[P::Item(I::ApplyStashWithIndex)])
    );
    assert_eq!(
        state.lookup(&maps.git_picker, "C-k".parse().unwrap()),
        Lookup::Actions(&[P::MoveDown])
    );
    assert_eq!(
        state.lookup(&maps.git_picker, "A-z".parse().unwrap()),
        Lookup::Unbound
    );
    assert_eq!(
        state.lookup(&maps.picker, "C-j".parse().unwrap()),
        Lookup::Actions(&[P::MoveUp])
    );
    assert_eq!(serde_json::to_value(P::Item(I::Stage)).unwrap(), "stage");
}

#[test]
fn history_picker_keys_inherit_navigation_and_only_accept_history_actions() {
    use crate::keymap::component::{PickerAction as P, PickerItemAction as I};
    use ui_core::keymap::{KeymapState, Lookup};
    let config = Config::load(
        Ok(&"[keys.history-picker]\nC-j = 'actions'".to_owned()),
        Ok("[keys.picker]\nC-j = 'move_up'\nC-k = 'move_down'".to_owned()),
    )
    .unwrap();
    let mut state = KeymapState::default();
    assert_eq!(
        state.lookup(
            &config.component_keys.history_picker,
            "C-j".parse().unwrap()
        ),
        Lookup::Actions(&[P::Item(I::Actions)])
    );
    assert_eq!(
        state.lookup(
            &config.component_keys.history_picker,
            "C-k".parse().unwrap()
        ),
        Lookup::Actions(&[P::MoveDown])
    );
    assert!(Config::load(
        Ok(&"[keys.history-picker]\nC-j = 'stage'".to_owned()),
        Ok(String::new())
    )
    .is_err());
}
