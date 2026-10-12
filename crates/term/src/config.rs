use crate::keymap;
use crate::keymap::component::{
    ComponentKeymaps, MenuAction, PageAction, PickerAction, PickerItemAction, PopupAction,
    PromptAction,
};
use crate::keymap::KeyTrie;
use loader::merge_toml_values;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use tokio::sync::mpsc::UnboundedSender;
use toml::de::Error as TomlError;
use ui_core::{keymap::Keymap, terminal::KittyKeyboardProtocolConfig};
use view::custom_commands::{CustomCommand, CustomCommands};
use view::{document::Mode, theme};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub component_keys: ComponentKeymaps,
    pub editor: view::config::Config,
    pub terminal: TerminalConfig,
}

/// Terminal capability overrides. Their TOML keys remain under `[editor]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct TerminalConfig {
    /// Override automatic detection of true color support.
    pub true_color: bool,
    /// Override automatic detection of extended underline support.
    pub undercurl: bool,
    /// Policy for enabling the Kitty keyboard protocol.
    pub kitty_keyboard_protocol: KittyKeyboardProtocolConfig,
}

/// The user-facing `[editor]` table, composed from editor and frontend settings.
/// This compatibility representation is also used by `:get`, `:set`, and `:toggle`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorSettings {
    #[serde(flatten)]
    pub editor: view::config::Config,
    #[serde(flatten)]
    pub terminal: TerminalConfig,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            editor: view::config::Config {
                clipboard_provider: crate::clipboard::default_provider(),
                ..Default::default()
            },
            terminal: TerminalConfig::default(),
        }
    }
}

impl<'de> Deserialize<'de> for EditorSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Partition before deserializing so both schemas retain strict unknown-field
        // validation. Flattened deserialization cannot preserve that boundary reliably.
        let mut editor = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let mut terminal = serde_json::Map::new();
        for key in ["true-color", "undercurl", "kitty-keyboard-protocol"] {
            if let Some(value) = editor.remove(key) {
                terminal.insert(key.to_owned(), value);
            }
        }
        if !editor.contains_key("clipboard-provider") {
            editor.insert(
                "clipboard-provider".into(),
                serde_json::to_value(crate::clipboard::default_provider())
                    .map_err(serde::de::Error::custom)?,
            );
        }
        Ok(Self {
            editor: serde_json::from_value(editor.into()).map_err(serde::de::Error::custom)?,
            terminal: serde_json::from_value(terminal.into()).map_err(serde::de::Error::custom)?,
        })
    }
}

/// Configuration requests originating in the terminal frontend.
pub enum ConfigEvent {
    Update(Box<EditorSettings>),
    Refresh,
}

/// A frontend configuration snapshot and the application's update queue.
/// Commands request updates; only the application installs them.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub current: &'a Config,
    pub updates: &'a UnboundedSender<ConfigEvent>,
}

/// The buffer picker accepts shared actions and its own item actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BufferPickerAction(PickerAction);

impl<'de> Deserialize<'de> for BufferPickerAction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::IntoDeserializer;
        let action = String::deserialize(deserializer)?;
        let action = match action.as_str() {
            "close_buffer" => PickerAction::Item(PickerItemAction::CloseBuffer),
            action => PickerAction::deserialize(action.into_deserializer())?,
        };
        Ok(Self(action))
    }
}

/// User-facing `[keys]` table keeps editor and component schemas separate.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Keys {
    normal: Option<keymap::KeyTrieNode>,
    insert: Option<keymap::KeyTrieNode>,
    select: Option<keymap::KeyTrieNode>,
    prompt: Keymap<PromptAction>,
    picker: Keymap<PickerAction>,
    buffer_picker: Keymap<BufferPickerAction>,
    menu: Keymap<MenuAction>,
    popup: Keymap<PopupAction>,
    hover: Keymap<PageAction>,
    signature_help: Keymap<PageAction>,
}

impl Keys {
    fn apply(
        self,
        editor: &mut HashMap<Mode, KeyTrie>,
        components: &mut ComponentKeymaps,
    ) -> Keymap<PickerAction> {
        for (mode, keys) in [
            (Mode::Normal, self.normal),
            (Mode::Insert, self.insert),
            (Mode::Select, self.select),
        ] {
            if let Some(keys) = keys {
                editor
                    .get_mut(&mode)
                    .unwrap()
                    .node_mut()
                    .unwrap()
                    .merge(keys);
            }
        }
        components.prompt.merge(self.prompt);
        components.picker.merge(self.picker);
        components.menu.merge(self.menu);
        components.popup.merge(self.popup);
        components.hover.merge(self.hover);
        components.signature_help.merge(self.signature_help);
        self.buffer_picker.map_actions(|action| action.0)
    }
}

fn resolve_keymaps(
    configured: impl IntoIterator<Item = Keys>,
) -> (HashMap<Mode, KeyTrie>, ComponentKeymaps) {
    let mut editor = keymap::default();
    let mut components = ComponentKeymaps::default();
    let mut buffer_overrides = Keymap::default();
    for keys in configured {
        buffer_overrides.merge(keys.apply(&mut editor, &mut components));
    }
    // Resolve inheritance after both configuration layers: buffer-specific
    // bindings take priority even over workspace changes to shared bindings.
    components.buffer_picker = components.picker.clone();
    components.buffer_picker.merge(buffer_overrides);
    (editor, components)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<Keys>,
    pub editor: Option<toml::Value>,
    commands: Option<Commands>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Commands {
    #[serde(flatten)]
    commands: BTreeMap<String, CustomCommand>,
}

impl Commands {
    fn merge(&mut self, other: Self) {
        self.commands.extend(other.commands);
    }

    fn into_custom_commands(self) -> CustomCommands {
        CustomCommands::new(
            self.commands
                .into_iter()
                .map(|(name, command)| command.named(name))
                .collect(),
        )
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: None,
            keys: keymap::default(),
            component_keys: ComponentKeymaps::default(),
            editor: EditorSettings::default().editor,
            terminal: TerminalConfig::default(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    BadConfig(TomlError),
    Error(IOError),
}

impl Default for ConfigLoadError {
    fn default() -> Self {
        ConfigLoadError::Error(IOError::new(std::io::ErrorKind::NotFound, "place holder"))
    }
}

impl Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigLoadError::BadConfig(err) => err.fmt(f),
            ConfigLoadError::Error(err) => err.fmt(f),
        }
    }
}

impl Config {
    pub fn editor_settings(&self) -> EditorSettings {
        EditorSettings {
            editor: self.editor.clone(),
            terminal: self.terminal.clone(),
        }
    }

    pub fn load(
        global: Result<&String, ConfigLoadError>,
        local: Result<String, ConfigLoadError>,
    ) -> Result<Config, ConfigLoadError> {
        let global_config: Result<ConfigRaw, ConfigLoadError> =
            global.and_then(|file| toml::from_str(file).map_err(ConfigLoadError::BadConfig));
        let local_config: Result<ConfigRaw, ConfigLoadError> =
            local.and_then(|file| toml::from_str(&file).map_err(ConfigLoadError::BadConfig));
        let res = match (global_config, local_config) {
            (Ok(mut global), Ok(local)) => {
                let (keys, component_keys) =
                    resolve_keymaps(global.keys.into_iter().chain(local.keys));

                let mut settings: EditorSettings = match (global.editor, local.editor) {
                    (None, None) => EditorSettings::default(),
                    (None, Some(val)) | (Some(val), None) => {
                        val.try_into().map_err(ConfigLoadError::BadConfig)?
                    }
                    (Some(global), Some(local)) => merge_toml_values(global, local, 3)
                        .try_into()
                        .map_err(ConfigLoadError::BadConfig)?,
                };

                if let Some(local_commands) = local.commands {
                    if let Some(global_commands) = &mut global.commands {
                        global_commands.merge(local_commands);
                    } else {
                        global.commands = Some(local_commands);
                    }
                }
                if let Some(commands) = global.commands {
                    settings.editor.commands = commands.into_custom_commands();
                }

                Config {
                    theme: local.theme.or(global.theme),
                    keys,
                    component_keys,
                    editor: settings.editor,
                    terminal: settings.terminal,
                }
            }
            // if any configs are invalid return that first
            (_, Err(ConfigLoadError::BadConfig(err)))
            | (Err(ConfigLoadError::BadConfig(err)), _) => {
                return Err(ConfigLoadError::BadConfig(err))
            }
            (Ok(config), Err(_)) | (Err(_), Ok(config)) => {
                let (keys, component_keys) = resolve_keymaps(config.keys);
                let mut settings: EditorSettings = config.editor.map_or_else(
                    || Ok(EditorSettings::default()),
                    |val| val.try_into().map_err(ConfigLoadError::BadConfig),
                )?;
                if let Some(commands) = config.commands {
                    settings.editor.commands = commands.into_custom_commands();
                }

                Config {
                    theme: config.theme,
                    keys,
                    component_keys,
                    editor: settings.editor,
                    terminal: settings.terminal,
                }
            }

            // these are just two io errors return the one for the global config
            (Err(err), Err(_)) => return Err(err),
        };

        Ok(res)
    }

    pub fn load_default() -> Result<Config, ConfigLoadError> {
        let global_config =
            fs::read_to_string(loader::config_file()).map_err(ConfigLoadError::Error)?;
        let local_config =
            fs::read_to_string(loader::workspace_config_file()).map_err(ConfigLoadError::Error);

        let phony_config = ConfigLoadError::Error(IOError::other("hacky placeholder"));
        let global_parsed = Config::load(Ok(&global_config), Err(phony_config))?;

        // We need to build a transient `WorkspaceTrust` just to ask whether the workspace is
        // trusted enough to load its `.mitos/config.toml`. The persisted-trust file on disk is the
        // source of truth either way; this transient instance has an empty cache and is dropped
        // after the check.
        let trust = loader::workspace_trust::WorkspaceTrust::new(
            (&global_parsed.editor.workspace_trust).into(),
        );
        if trust
            .query_current(loader::workspace_trust::TrustQuery::LocalConfig)
            .is_trusted()
        {
            let mut merged = Config::load(Ok(&global_config), local_config)?;
            // editor.workspace-trust is global/user-scope only. Without this override, a
            // workspace's `.mitos/config.toml` could set `level = "insecure"`; once the user trusted
            // *that* workspace, refresh_config would re-load with the override merged in and from
            // then on every subsequent workspace in the session would be implicitly trusted. Pin
            // the gate's own configuration to the global file.
            merged.editor.workspace_trust = global_parsed.editor.workspace_trust;
            Ok(merged)
        } else {
            Ok(global_parsed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::merge_keys;

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
            "[keys.menu]\nC-x = 'close_buffer'",
            "[keys.buffer-picker]\nC-x = 'scroll_down'",
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
                kitty_keyboard_protocol: KittyKeyboardProtocolConfig::Enabled,
            }
        );
        assert_eq!(config.editor.scrolloff, 7);
        assert!(config.editor.auto_save.after_delay.enable);
        assert_eq!(config.editor.auto_save.after_delay.timeout, 2500);

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
        assert_eq!(value["true-color"], true);
        assert_eq!(value["idle-timeout"], 123);
        assert_eq!(value["cursor-shape"]["insert"], "bar");
        assert_eq!(
            serde_json::from_value::<EditorSettings>(value).unwrap(),
            settings
        );
    }

    #[test]
    fn icons_are_controlled_by_one_editor_option() {
        assert!(!Config::load_test("").editor.icons);
        assert!(Config::load_test("[editor]\nicons = true").editor.icons);
    }

    #[test]
    fn breadcrumbs_are_disabled_by_default_and_configurable() {
        use view::config::BreadcrumbPathOptions;

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
        use view::clipboard::ClipboardProvider;
        let global = "[editor]\nclipboard-provider = 'termcode'".to_owned();
        let local = "[editor]\nclipboard-provider = 'none'".to_owned();
        let config = Config::load(Ok(&global), Ok(local)).unwrap();
        assert_eq!(config.editor.clipboard_provider, ClipboardProvider::None);
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
            serialized["clipboard-provider"]["custom"]["yank"]["command"],
            "read-clipboard"
        );
        assert_eq!(
            serialized["clipboard-provider"]["custom"]["paste"]["args"],
            serde_json::json!([])
        );
        assert_eq!(
            serde_json::from_value::<EditorSettings>(serialized).unwrap(),
            settings
        );
        assert!(Config::load_test_result("[editor]\nclipboard-provider = 'unknown'").is_err());
    }
}
