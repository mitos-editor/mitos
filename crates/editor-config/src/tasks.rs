use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// A named shell task configured under `[editor.tasks.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TaskConfig {
    /// Command passed to the configured editor shell.
    pub command: String,
    /// Working directory, relative to the workspace root unless absolute.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Environment variables added to or overriding the inherited environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Optional location regex with named `path`, `line`, and optional `column` captures.
    #[serde(default)]
    pub error_pattern: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn task_defaults_and_workspace_table_round_trip() {
        assert!(Config::default().tasks.is_empty());
        let config: Config = toml::from_str(
            r#"
            [tasks.build]
            command = "cargo build --color never"

            [tasks.test]
            command = "go test ./..."
            cwd = "services/api"
            env = { GOFLAGS = "-count=1" }
            error-pattern = '^(?P<path>.+):(?P<line>\d+):(?P<column>\d+): (?P<message>.*)$'
            "#,
        )
        .unwrap();

        let build = &config.tasks["build"];
        assert_eq!(build.command, "cargo build --color never");
        assert_eq!(build.cwd, None);
        assert!(build.env.is_empty());
        assert_eq!(build.error_pattern, None);

        let test = &config.tasks["test"];
        assert_eq!(
            test.cwd.as_deref(),
            Some(std::path::Path::new("services/api"))
        );
        assert_eq!(test.env["GOFLAGS"], "-count=1");
        assert!(test.error_pattern.as_deref().unwrap().contains("(?P<path>"));
        let serialized = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&serialized).unwrap(), config);
    }

    #[test]
    fn task_rejects_missing_command_unknown_fields_and_invalid_types() {
        for source in [
            "",
            "command = 1",
            "command = 'make'\nunknown = true",
            "command = 'make'\ncwd = false",
            "command = 'make'\nenv = []",
            "command = 'make'\nenv = { JOBS = 4 }",
            "command = 'make'\nerror-pattern = false",
            "command = 'make'\nerror_pattern = '.*'",
        ] {
            assert!(toml::from_str::<TaskConfig>(source).is_err(), "{source}");
        }
    }
}
