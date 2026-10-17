use crate::keymap;
use crate::keymap::component::{
    ComponentKeymaps, MenuAction, PageAction, PickerAction, PickerItemAction, PopupAction,
    PromptAction,
};
use crate::keymap::KeyTrie;
pub use crate::terminal_launcher::TerminalLauncherConfig;
use loader::config::migration;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use tokio::sync::mpsc::UnboundedSender;
use toml::de::Error as TomlError;
use ui_core::{keymap::Keymap, terminal::KittyKeyboardProtocolConfig};
use view::custom_commands::{CustomCommand, CustomCommands};
use view::document::Mode;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub component_keys: ComponentKeymaps,
    pub editor: view::config::Config,
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
    pub editor: view::config::Config,
    pub terminal: TerminalConfig,
}

impl Default for EditorSettings {
    fn default() -> Self {
        let mut editor = view::config::Config::default();
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
        let editor: view::config::Config =
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
    pub terminal: Option<toml::Value>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationScope {
    User,
    Workspace,
}

/// Notices from explicitly requested configuration migration.
#[derive(Debug)]
pub enum ConfigMigration {
    Updated {
        path: std::path::PathBuf,
        backup: std::path::PathBuf,
    },
    Warning(String),
}

fn migrate_file(
    path: &std::path::Path,
    global: &str,
    local: Option<&str>,
    is_user: bool,
) -> anyhow::Result<Option<migration::FileMigration>> {
    migration::file(path, |source| {
        // Project tables can inherit required fields from the user layer.
        // Validate the candidate in the same context in which it was loaded.
        let global = if is_user { source } else { global }.to_owned();
        let local = if is_user { local } else { Some(source) }
            .map(str::to_owned)
            .ok_or_else(ConfigLoadError::default);
        Config::load(Ok(&global), local)
            .map(|_| ())
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    })
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

    /// Load user and trusted workspace configuration without rewriting files.
    pub fn load_default() -> Result<Config, ConfigLoadError> {
        Self::load_default_impl(None).map(|(config, _)| config)
    }

    /// Persist known renames in the selected configuration scope.
    /// Write failures become notices while loaded legacy settings remain usable.
    pub fn migrate(scope: MigrationScope) -> anyhow::Result<Vec<ConfigMigration>> {
        let path = Self::migration_path(scope)?;
        fs::read_to_string(&path)?;
        Self::load_default_impl(Some(scope))
            .map(|(_, notices)| notices)
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    }

    /// Check the loaded configuration and trust policy before offering a rewrite.
    pub fn migration_path(scope: MigrationScope) -> anyhow::Result<std::path::PathBuf> {
        let config = Self::load_default().map_err(|err| anyhow::anyhow!(err.to_string()))?;
        match scope {
            MigrationScope::User => Ok(loader::config_file()),
            MigrationScope::Workspace => {
                use workspace::trust::{TrustQuery, WorkspaceTrust};
                let trust = WorkspaceTrust::new(
                    (&config.editor.workspace_trust).into(),
                    loader::workspace_trust_dir(),
                );
                anyhow::ensure!(
                    trust.query_current(TrustQuery::LocalConfig).is_trusted(),
                    "Workspace configuration is not trusted; review and trust it before migrating"
                );
                Ok(loader::workspace_config_file())
            }
        }
    }

    fn load_default_impl(
        scope: Option<MigrationScope>,
    ) -> Result<(Config, Vec<ConfigMigration>), ConfigLoadError> {
        use workspace::trust::{TrustQuery, TrustStatus, WorkspaceTrust};
        let user_path = loader::config_file();
        let (global_config, user_exists) = match fs::read_to_string(&user_path) {
            Ok(source) => (source, true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(err) => return Err(ConfigLoadError::Error(err)),
        };
        let global_parsed = Config::load(Ok(&global_config), Err(ConfigLoadError::default()))?;
        let trust_config = (&global_parsed.editor.workspace_trust).into();
        let trust = WorkspaceTrust::new(trust_config, loader::workspace_trust_dir());
        let workspace = workspace::find_workspace().0;
        let project_path = workspace.join(".mitos/config.toml");
        let mut paths = Vec::new();
        if user_exists {
            paths.push((user_path, true));
        }

        let mut local_source = None;
        // Check user-scope trust before even reading the project config.
        let config = if trust
            .query(&workspace, TrustQuery::LocalConfig)
            .is_trusted()
        {
            let local = fs::read_to_string(&project_path).map_err(ConfigLoadError::Error);
            if let Ok(source) = &local {
                local_source = Some(source.clone());
                paths.push((project_path, false));
            }
            let mut merged = Config::load(Ok(&global_config), local)?;
            // A project cannot change the policy used to decide whether to load it.
            merged.editor.workspace_trust = global_parsed.editor.workspace_trust;
            merged
        } else {
            global_parsed
        };
        let mut notices = Vec::new();
        if let Some(scope) = scope {
            // All loaded layers must validate before either file is rewritten.
            for (path, is_user) in paths {
                if is_user != (scope == MigrationScope::User) {
                    continue;
                }
                match migrate_file(&path, &global_config, local_source.as_deref(), is_user) {
                    Ok(Some(written)) => {
                        // Never create a grant for implicit trust. Preserve an existing
                        // grant only when the exact migration accounts for the changes.
                        if trust.status(&workspace) == TrustStatus::Trusted
                            && !trust.record_config_migration(
                                &workspace,
                                &workspace::trust::ConfigMigration {
                                    target: &written.target,
                                    backup: &written.backup,
                                    original: &written.original,
                                    updated: &written.updated,
                                },
                            )
                            && WorkspaceTrust::new(
                                (&config.editor.workspace_trust).into(),
                                loader::workspace_trust_dir(),
                            )
                            .status(&workspace)
                                != TrustStatus::Trusted
                        {
                            notices.push(ConfigMigration::Warning(
                                "Workspace trust could not be preserved; review the changes and trust the workspace again".into()
                            ));
                        }
                        log::info!(
                            "Updated {}; backup: {}",
                            path.display(),
                            written.backup.display()
                        );
                        notices.push(ConfigMigration::Updated {
                            path,
                            backup: written.backup,
                        });
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let message = format!(
                            "Could not update {}; using loaded settings: {err:#}. For managed or read-only files, export with ms --migrate-config <file> --stdout and update the configuration source",
                            path.display()
                        );
                        log::warn!("{message}");
                        notices.push(ConfigMigration::Warning(message));
                    }
                }
            }
        }
        Ok((config, notices))
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
        assert!(
            Config::load_test_result("[keys.normal]\nlabel = 'Save'\ncommand = ':write'").is_err()
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
        use view::config::InlineBlameShow;

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
        assert!(config.editor.rulers.is_empty());
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
        let config = Config::load_test("[editor.word-completion]\nenable = false\n[editor.completion.word]\ntrigger-length = 3");
        assert!(!config.editor.completion.word.enable);
        assert_eq!(config.editor.completion.word.trigger_length.get(), 3);
        let config = Config::load_test(
            "[editor]\nword-completion = {}\n[editor.completion.word]\nenable = false",
        );
        assert!(!config.editor.completion.word.enable);
    }
}
