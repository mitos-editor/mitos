//! Frontend configuration values and strict serialized schemas.
use super::{keymaps::Keys, TerminalLauncherConfig};
use crate::keymap::{self, component::ComponentKeymaps, KeyTrie};
use editor_config::{CustomCommand, CustomCommands};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap};
use tokio::sync::mpsc::UnboundedSender;
use ui_core::terminal::KittyKeyboardProtocolConfig;
use view::document::Mode;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub component_keys: ComponentKeymaps,
    pub editor: editor_config::Config,
    pub terminal: TerminalConfig,
}

/// Frontend-owned terminal settings from the top-level `[terminal]` table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct TerminalConfig {
    /// Override automatic detection of true color support.
    pub true_color: bool,
    /// Override automatic detection of extended underline support.
    pub undercurl: bool,
    /// Policy for enabling the Kitty keyboard protocol.
    pub keyboard: KeyboardConfig,
    pub launcher: Option<TerminalLauncherConfig>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            true_color: false,
            undercurl: false,
            keyboard: KeyboardConfig::default(),
            launcher: crate::terminal_launcher::default_launcher(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct KeyboardConfig {
    pub kitty_protocol: KittyKeyboardProtocolConfig,
}

/// Runtime command settings: editor-relative paths and `terminal.*` paths.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorSettings {
    #[serde(flatten)]
    pub editor: editor_config::Config,
    pub terminal: TerminalConfig,
}

impl Default for EditorSettings {
    fn default() -> Self {
        let mut editor = editor_config::Config::default();
        editor.clipboard.provider = crate::clipboard::default_provider();
        Self {
            editor,
            terminal: TerminalConfig::default(),
        }
    }
}

impl<'de> Deserialize<'de> for EditorSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Partition frontend settings before deserializing to preserve strict
        // unknown-field validation on both sides of the ownership boundary.
        let mut editor = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let terminal: TerminalConfig = editor
            .remove("terminal")
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .unwrap_or_default();
        let clipboard = editor
            .entry("clipboard".to_owned())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(clipboard) = clipboard.as_object_mut() {
            clipboard
                .entry("provider".to_owned())
                .or_insert_with(|| serde_json::json!(crate::clipboard::default_provider()));
        }
        let editor: editor_config::Config =
            serde_json::from_value(editor.into()).map_err(serde::de::Error::custom)?;
        Ok(Self { editor, terminal })
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<Keys>,
    pub editor: Option<toml::Value>,
    pub terminal: Option<toml::Value>,
    pub(super) commands: Option<Commands>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct Commands {
    #[serde(flatten)]
    commands: BTreeMap<String, CustomCommand>,
}

impl Commands {
    pub(super) fn merge(&mut self, other: Self) {
        self.commands.extend(other.commands);
    }

    pub(super) fn into_custom_commands(self) -> CustomCommands {
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
