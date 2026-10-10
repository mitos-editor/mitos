//! Configuration and package admission. All disk reads and manifest validation
//! run before a package reaches the command registry or a guest worker.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::{ensure, Context, Result};
use plugin_api::{CapabilitySet, Event, Permissions, ABI_VERSION};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{assets, compatibility, component, filesystem, policy};

const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// Configuration for a single plugin. Relative paths use the editor config directory.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PluginConfig {
    /// The plugin manifest (`plugin.toml`), or its containing directory.
    pub path: PathBuf,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
    /// Authority granted only by the user's global configuration.
    #[serde(default)]
    pub permissions: Permissions,
    /// Optional module digest pin. Workspace config cannot replace this pin.
    #[serde(default)]
    pub sha256: Option<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            enabled: true,
            config: Value::Null,
            permissions: Permissions::default(),
            sha256: None,
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Manifest {
    #[serde(default = "manifest_version")]
    manifest_version: u32,
    #[serde(default = "api_version")]
    api_version: String,
    #[serde(default)]
    minimum_host_version: Option<String>,
    abi_version: u32,
    #[serde(default)]
    module: Option<PathBuf>,
    #[serde(default = "default_capabilities")]
    capabilities: CapabilitySet,
    #[serde(default)]
    optional_capabilities: CapabilitySet,
    #[serde(default)]
    required_capabilities: CapabilitySet,
    #[serde(default)]
    commands: BTreeMap<String, CommandMetadata>,
    #[serde(default)]
    events: Vec<Event>,
    #[serde(default)]
    contributions: plugin_api::assets::Contributions,
}

fn default_capabilities() -> CapabilitySet {
    Permissions::default().capabilities
}

fn manifest_version() -> u32 {
    compatibility::MANIFEST_VERSION
}
fn api_version() -> String {
    compatibility::API_VERSION.to_owned()
}

/// Static documentation and positional arguments for a registered command.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandMetadata {
    pub doc: String,
    #[serde(default)]
    pub arguments: plugin_api::commands::CommandArguments,
}

pub(super) struct Package {
    pub(super) commands: BTreeMap<String, CommandMetadata>,
    pub(super) events: Vec<Event>,
    pub(super) config: Arc<str>,
    pub(super) policy: Arc<policy::AccessPolicy>,
    pub(super) bytes: Option<Arc<[u8]>>,
    pub(super) assets: Arc<plugin_api::assets::OwnedAssets>,
    pub(super) asset_bytes: usize,
}
impl Package {
    pub(super) fn load(
        name: &str,
        config: &PluginConfig,
        base: &Path,
        generation: u64,
    ) -> Result<Self> {
        let path = base.join(&config.path);
        let manifest_path = if path.is_dir() {
            path.join("plugin.toml")
        } else {
            path
        };
        let manifest_path = manifest_path
            .canonicalize()
            .with_context(|| format!("opening manifest {}", manifest_path.display()))?;
        let directory = manifest_path
            .parent()
            .context("manifest has no parent directory")?;
        let package = filesystem::ScopedDirectory::open(directory)?;
        let manifest_bytes = package.read_bounded(
            Path::new(
                manifest_path
                    .file_name()
                    .context("manifest has no filename")?,
            ),
            MAX_MANIFEST_BYTES,
        )?;
        let mut manifest: Manifest = toml::from_str(std::str::from_utf8(&manifest_bytes)?)
            .context("invalid plugin manifest")?;
        compatibility::validate(
            manifest.manifest_version,
            &manifest.api_version,
            manifest.minimum_host_version.as_deref(),
        )?;
        manifest
            .capabilities
            .extend(manifest.optional_capabilities.iter().copied());
        ensure!(
            manifest
                .required_capabilities
                .is_subset(&manifest.capabilities),
            "required capabilities must also be declared"
        );
        ensure!(
            manifest.commands.len() <= 64 && manifest.events.len() <= 32,
            "plugin manifest exceeds command or subscription limits"
        );
        for command in manifest.commands.values_mut() {
            command
                .arguments
                .validate()
                .context("invalid command arguments")?;
            ensure!(
                command.doc.len() <= 4096,
                "command documentation exceeds 4096 bytes"
            );
            command.doc = plugin_api::ui::terminal_text(&command.doc, false);
        }
        let policy = Arc::new(policy::AccessPolicy::new(
            manifest.capabilities.clone(),
            config.permissions.clone(),
            base,
        )?);
        for capability in &manifest.required_capabilities {
            policy.require(*capability)?;
        }
        ensure!(
            manifest.abi_version == ABI_VERSION,
            "unsupported ABI version {} (expected {ABI_VERSION})",
            manifest.abi_version
        );
        ensure!(
            manifest.commands.keys().all(|name| valid_identifier(name)),
            "invalid command name (use letters, digits, '-' or '_')"
        );
        let prepared_config: Arc<str> = component::bounded_json(&config.config, 64 * 1024)?.into();
        let (assets, asset_bytes) =
            assets::load(&package, &manifest.contributions, name, generation)?;
        let bytes = if let Some(module) = &manifest.module {
            ensure!(
                !module.as_os_str().is_empty()
                    && module
                        .components()
                        .all(|part| matches!(part, Component::Normal(_))),
                "module must be a relative path within the plugin directory"
            );
            let bytes = package.read_bounded(module, component::MAX_COMPONENT_BYTES)?;
            ensure!(
                bytes.starts_with(b"\0asm"),
                "module must be a WebAssembly binary component"
            );
            if let Some(pin) = &config.sha256 {
                ensure!(
                    pin.len() == 64 && pin.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "sha256 must contain 64 hexadecimal digits"
                );
                let digest: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                ensure!(
                    digest.eq_ignore_ascii_case(pin),
                    "module SHA-256 does not match the user's package pin"
                );
            }
            Some(bytes.into())
        } else {
            ensure!(
                manifest.commands.is_empty() && manifest.events.is_empty(),
                "commands and event hooks require a component module"
            );
            ensure!(
                config.sha256.is_none(),
                "a module digest pin requires a component module"
            );
            None
        };
        Ok(Self {
            commands: manifest.commands,
            events: manifest.events,
            config: prepared_config,
            policy,
            bytes,
            assets: Arc::new(assets),
            asset_bytes,
        })
    }
}

pub(super) fn valid_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
