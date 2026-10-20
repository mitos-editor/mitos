//! Parse and compose user and workspace layers without filesystem effects.
use super::{keymaps::resolve_keymaps, Config, ConfigRaw, EditorSettings};
use loader::config::migration;
use std::{fmt::Display, io::Error as IOError};
use toml::de::Error as TomlError;

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

fn parse_source(source: &str) -> Result<ConfigRaw, TomlError> {
    let mut value = toml::from_str(source)?;
    migration::config(&mut value)?;
    value.try_into()
}

fn settings_from_sources(
    global_editor: Option<toml::Value>,
    global_terminal: Option<toml::Value>,
    local_editor: Option<toml::Value>,
    local_terminal: Option<toml::Value>,
) -> Result<EditorSettings, ConfigLoadError> {
    let table = || toml::Value::Table(Default::default());
    let mut editor = migration::merge(
        global_editor.unwrap_or_else(table),
        local_editor.unwrap_or_else(table),
    );
    let terminal = migration::merge(
        global_terminal.unwrap_or_else(table),
        local_terminal.unwrap_or_else(table),
    );
    if let Some(editor) = editor.as_table_mut() {
        editor.insert("terminal".into(), terminal);
    }
    editor.try_into().map_err(ConfigLoadError::BadConfig)
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
            global.and_then(|file| parse_source(file).map_err(ConfigLoadError::BadConfig));
        let local_config: Result<ConfigRaw, ConfigLoadError> =
            local.and_then(|file| parse_source(&file).map_err(ConfigLoadError::BadConfig));
        let res = match (global_config, local_config) {
            (Ok(mut global), Ok(local)) => {
                let (keys, component_keys) =
                    resolve_keymaps(global.keys.into_iter().chain(local.keys));

                let mut settings = settings_from_sources(
                    global.editor,
                    global.terminal,
                    local.editor,
                    local.terminal,
                )?;

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
                let mut settings =
                    settings_from_sources(config.editor, config.terminal, None, None)?;
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
}
