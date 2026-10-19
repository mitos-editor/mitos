//! Mitos launch settings, separate from DAP wire payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// How Mitos starts and communicates with a configured adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DebugAdapterTransport {
    Stdio,
    Tcp,
}

/// An interactive value requested while completing a debug template.
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AdvancedCompletion {
    pub name: Option<String>,
    pub completion: Option<String>,
    pub default: Option<String>,
}

/// A completion prompt used by a [`DebugTemplate`].
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", untagged)]
pub enum DebugConfigCompletion {
    Named(String),
    Advanced(AdvancedCompletion),
}

/// A named launch or attach template exposed to users.
///
/// `args` is deliberately untyped because its contents are adapter-specific
/// and become the arguments of the template's DAP request.
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DebugTemplate {
    pub name: String,
    pub request: dap_types::ConnectionType,
    #[serde(default)]
    pub completion: Vec<DebugConfigCompletion>,
    pub args: HashMap<String, Value>,
}

/// Configuration for starting and communicating with a debug adapter.
///
/// `transport` currently accepts `"stdio"` or `"tcp"`. For TCP adapters,
/// `port_arg` is a format string whose `{}` placeholder is replaced with an
/// ephemeral local port before the process is spawned.
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DebugAdapterConfig {
    pub name: String,
    pub transport: DebugAdapterTransport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub port_arg: Option<String>,
    pub templates: Vec<DebugTemplate>,
    #[serde(default)]
    pub quirks: DebuggerQuirks,
}

/// Workarounds for differences between debug adapters.
#[derive(Debug, Default, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct DebuggerQuirks {
    /// Treat paths received from the adapter as absolute.
    #[serde(default)]
    pub absolute_paths: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
name = "example"
transport = "stdio"
command = "example-dap"
[[templates]]
name = "run"
request = "launch"
args = { program = "{0}" }
completion = ["filename", { name = "argument", default = "value" }]
"#;

    #[test]
    fn launch_settings_keep_the_existing_toml_format() {
        let config: DebugAdapterConfig = toml::from_str(CONFIG).unwrap();
        assert_eq!(config.transport, DebugAdapterTransport::Stdio);
        assert_eq!(
            config.templates[0].request,
            dap_types::ConnectionType::Launch
        );
        assert_eq!(config.templates[0].args["program"], "{0}");
        assert_eq!(
            config,
            toml::from_str(&toml::to_string(&config).unwrap()).unwrap()
        );
    }

    #[test]
    fn unsupported_transport_and_request_names_are_rejected() {
        for (from, to) in [("stdio", "unknown"), ("launch", "unknown")] {
            assert!(toml::from_str::<DebugAdapterConfig>(&CONFIG.replace(from, to)).is_err());
        }
    }
}
