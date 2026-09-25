use crate::keymap;
use crate::keymap::{merge_keys, KeyTrie};
use loader::merge_toml_values;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use tokio::sync::mpsc::UnboundedSender;
use toml::de::Error as TomlError;
use ui_core::terminal::KittyKeyboardProtocolConfig;
use view::custom_commands::{CustomCommand, CustomCommands};
use view::{document::Mode, theme};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EditorSettings {
    #[serde(flatten)]
    pub editor: view::config::Config,
    #[serde(flatten)]
    pub terminal: TerminalConfig,
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<HashMap<Mode, KeyTrie>>,
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
            editor: view::config::Config::default(),
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
                let mut keys = keymap::default();
                if let Some(global_keys) = global.keys {
                    merge_keys(&mut keys, global_keys)
                }
                if let Some(local_keys) = local.keys {
                    merge_keys(&mut keys, local_keys)
                }

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
                let mut keys = keymap::default();
                if let Some(keymap) = config.keys {
                    merge_keys(&mut keys, keymap);
                }
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
        use view::editor::BreadcrumbPathOptions;

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
}
