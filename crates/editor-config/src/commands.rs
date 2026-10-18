use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Deserializer};

/// The configured custom command set.
///
/// This wraps an [`Arc`], so carrying it across configuration updates is cheap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomCommands(Arc<BTreeMap<String, CustomCommand>>);

impl CustomCommands {
    pub fn new(commands: Vec<CustomCommand>) -> Self {
        Self(Arc::new(
            commands
                .into_iter()
                .map(|command| (command.name.clone(), command))
                .collect(),
        ))
    }

    pub fn get(&self, name: &str) -> Option<&CustomCommand> {
        let name = name.trim_start_matches(':');
        self.0.get(name)
    }

    pub fn visible_names(&self) -> impl Iterator<Item = &str> {
        self.0
            .values()
            .filter(|command| !command.hidden)
            .map(|command| command.name.as_str())
    }
}

/// A user-defined command-mode command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomCommand {
    pub name: String,
    pub description: Option<String>,
    pub commands: Vec<String>,
    pub accepts: Option<String>,
    pub completer: Option<String>,
    pub hidden: bool,
}

impl CustomCommand {
    /// Prefix used to bypass a custom command that shadows a built-in command.
    pub const ESCAPE: char = '^';

    pub fn named(mut self, name: String) -> Self {
        self.hidden = !name.starts_with(':');
        self.name = name.trim_start_matches(':').to_owned();
        self
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CustomCommandDefinition {
    Command(String),
    Sequence(Vec<String>),
    Detailed(DetailedCustomCommand),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailedCustomCommand {
    commands: Vec<String>,
    #[serde(rename = "desc")]
    description: Option<String>,
    accepts: Option<String>,
    completer: Option<String>,
}

impl<'de> Deserialize<'de> for CustomCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let definition = CustomCommandDefinition::deserialize(deserializer)?;
        let command = match definition {
            CustomCommandDefinition::Command(command) => Self {
                commands: vec![command],
                ..Self::default()
            },
            CustomCommandDefinition::Sequence(commands) => Self {
                commands,
                ..Self::default()
            },
            CustomCommandDefinition::Detailed(details) => Self {
                commands: details.commands,
                description: details.description,
                accepts: details.accepts,
                completer: details
                    .completer
                    .map(|command| command.trim_start_matches(':').to_owned()),
                ..Self::default()
            },
        };

        let macro_count = command
            .commands
            .iter()
            .filter(|command| command.starts_with('@'))
            .count();
        if macro_count > 1 || (macro_count == 1 && command.commands.len() > 1) {
            return Err(serde::de::Error::custom(
                "macro keybindings may not be used in command sequences",
            ));
        }

        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_all_command_forms() {
        let simple: CustomCommand = toml::from_str("commands = ':write'")
            .map(|value: DetailedWrapper| value.commands)
            .unwrap();
        assert_eq!(simple.commands, [":write"]);

        let detailed: CustomCommand = toml::from_str(
            "commands = [':write %arg{0}']\ndesc = 'Save a path'\naccepts = '<path>'\ncompleter = ':write'",
        )
        .unwrap();
        assert_eq!(detailed.completer.as_deref(), Some("write"));
        assert_eq!(detailed.accepts.as_deref(), Some("<path>"));
        assert_eq!(detailed.description.as_deref(), Some("Save a path"));
    }

    #[derive(Deserialize)]
    struct DetailedWrapper {
        commands: CustomCommand,
    }

    #[test]
    fn rejects_a_macro_in_a_sequence() {
        let error = toml::from_str::<CustomCommand>("commands = ['@x', ':write']").unwrap_err();
        assert!(error
            .to_string()
            .contains("macro keybindings may not be used in command sequences"));
    }
}
