//! Capability-checked component packages, command registry, and async execution.
//!
//! Package preparation performs filesystem work, compilation, and linking off the
//! editor path. Activation is transactional; every actor belongs to one editor
//! generation and stages effects until the owning editor applies them.

use anyhow::{ensure, Context, Result};
use plugin_api::{
    Capability, CapabilitySet, EditorContext, ErrorCode, Event, HostServices, Permissions, Request,
    ServiceError, ABI_VERSION,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context as TaskContext, Poll},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub const MAX_MESSAGE_BYTES: usize = component::MAX_MESSAGE_BYTES;
const MAX_MODULE_BYTES: usize = component::MAX_COMPONENT_BYTES;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_PACKAGE_SET_BYTES: usize = 64 * 1024 * 1024;
const MAX_PLUGINS: usize = 32;
mod complexity;
mod component;
pub mod filesystem;
pub mod native;
pub mod policy;
mod worker;
pub use worker::{CompletedResponse, Completion, InvocationTarget, WorkerPool};

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
    abi_version: u32,
    #[serde(default)]
    module: Option<PathBuf>,
    #[serde(default = "default_capabilities")]
    capabilities: CapabilitySet,
    #[serde(default)]
    commands: BTreeMap<String, Command>,
    #[serde(default)]
    events: Vec<Event>,
}

fn default_capabilities() -> CapabilitySet {
    Permissions::default().capabilities
}

fn observes_editor_state(event: Event) -> bool {
    matches!(
        event,
        Event::DocumentOpened
            | Event::DocumentChanged
            | Event::DocumentSaved
            | Event::DocumentClosed
            | Event::SelectionChanged
            | Event::PostCommand
            | Event::PostInsertChar
            | Event::DocumentFocusLost
            | Event::State
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    doc: String,
    #[serde(default)]
    arguments: plugin_api::commands::CommandArguments,
}

/// A command registered by a plugin, qualified by its configured name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    pub name: String,
    pub doc: String,
    pub arguments: plugin_api::commands::CommandArguments,
}

/// The command registry boundary carried over from Helix's plugin proposal.
pub trait PluginSystem {
    fn available_commands(&self) -> Vec<PluginCommand>;
    fn get_doc_for_identifier(&self, identifier: &str) -> Option<String>;
}

/// Packages and serialized actors belonging to one editor generation.
#[derive(Default)]
pub struct PluginManager {
    plugins: BTreeMap<String, Plugin>,
    pool: Option<WorkerPool>,
    preparing: Arc<AtomicBool>,
    revoked: CancellationToken,
    generation: u64,
}

/// Owned asynchronous preparation. Dropping it cancels pending worker admission;
/// an already running native compile remains bounded but cannot be interrupted.
pub struct ManagerPreparation {
    receiver: oneshot::Receiver<Result<PreparedManager, ServiceError>>,
    cancel: CancellationToken,
}
impl Future for ManagerPreparation {
    type Output = Result<PreparedManager, ServiceError>;
    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(ServiceError::new(
                ErrorCode::HostFailure,
                "plugin package loader stopped",
            ))),
        }
    }
}
impl Drop for ManagerPreparation {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
struct PreparationAdmission(Arc<AtomicBool>);
impl Drop for PreparationAdmission {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct PreparedManager {
    packages: BTreeMap<String, PreparedPackage>,
    pool: Option<WorkerPool>,
    preparing: Arc<AtomicBool>,
    generation: u64,
}
struct PreparedPackage {
    package: Package,
    instance: Option<worker::PreparedPlugin>,
}
impl Drop for PreparedManager {
    fn drop(&mut self) {
        for package in self.packages.values() {
            package.package.policy.revoke();
        }
    }
}
impl PreparedManager {
    /// Move validated stores and metadata into a generation. No guest is invoked.
    /// Any activation failure revokes the partial replacement before returning.
    pub fn activate(mut self) -> Result<PluginManager, ServiceError> {
        let mut manager = PluginManager {
            plugins: BTreeMap::new(),
            pool: self.pool.take(),
            preparing: self.preparing.clone(),
            generation: self.generation,
            revoked: CancellationToken::new(),
        };
        for (name, prepared) in std::mem::take(&mut self.packages) {
            let actor = prepared
                .instance
                .map(|instance| manager.pool.as_ref().unwrap().activate(instance))
                .transpose()?;
            manager.plugins.insert(
                name,
                Plugin {
                    manifest: prepared.package.manifest,
                    config: prepared.package.config,
                    policy: prepared.package.policy,
                    actor,
                },
            );
        }
        Ok(manager)
    }
}

impl PluginManager {
    /// Validate the complete replacement off-thread. A failure never mutates this
    /// manager; the caller retains it until successful activation and swapping.
    pub fn prepare(
        &self,
        configs: BTreeMap<String, PluginConfig>,
        base: PathBuf,
        generation: u64,
    ) -> Result<ManagerPreparation, ServiceError> {
        if self.revoked.is_cancelled() {
            return Err(ServiceError::new(
                ErrorCode::Cancelled,
                "plugin manager has been revoked",
            ));
        }
        self.preparing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                ServiceError::new(
                    ErrorCode::ResourceExhausted,
                    "plugin replacement is already being prepared",
                )
            })?;
        let admission = PreparationAdmission(self.preparing.clone());
        let pool = self.pool.clone();
        let preparing = self.preparing.clone();
        let cancel = self.revoked.child_token();
        let stopping = cancel.clone();
        let (send, receiver) = oneshot::channel();
        if configs.values().all(|config| !config.enabled) {
            drop(admission);
            let _ = send.send(Ok(PreparedManager {
                packages: BTreeMap::new(),
                pool: None,
                preparing,
                generation,
            }));
            return Ok(ManagerPreparation { receiver, cancel });
        }
        std::thread::Builder::new()
            .name("plugin-packages".into())
            .spawn(move || {
                let result = prepare_packages(configs, base, generation, pool, preparing, stopping);
                drop(admission);
                let _ = send.send(result);
            })
            .map_err(|cause| {
                ServiceError::new(
                    ErrorCode::HostFailure,
                    format!("spawning plugin package loader: {cause}"),
                )
            })?;
        Ok(ManagerPreparation { receiver, cancel })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn available_commands(&self) -> Vec<PluginCommand> {
        self.plugins
            .iter()
            .filter(|(_, plugin)| plugin.active())
            .flat_map(|(name, plugin)| {
                plugin
                    .manifest
                    .commands
                    .iter()
                    .map(move |(command, details)| PluginCommand {
                        name: format!("{name}.{command}"),
                        doc: details.doc.clone(),
                        arguments: details.arguments.clone(),
                    })
            })
            .collect()
    }
    pub fn get_doc_for_identifier(&self, identifier: &str) -> Option<String> {
        let (name, command) = identifier.split_once('.')?;
        let plugin = self.plugins.get(name)?;
        plugin
            .active()
            .then(|| {
                plugin
                    .manifest
                    .commands
                    .get(command)
                    .map(|command| command.doc.clone())
            })
            .flatten()
    }
    pub fn policy(&self, name: &str) -> Option<Arc<policy::AccessPolicy>> {
        self.plugins.get(name).map(|plugin| plugin.policy.clone())
    }
    pub fn get_arguments_for_identifier(
        &self,
        identifier: &str,
    ) -> Option<plugin_api::commands::CommandArguments> {
        let (name, command) = identifier.split_once('.')?;
        let plugin = self.plugins.get(name)?;
        plugin
            .active()
            .then(|| {
                plugin
                    .manifest
                    .commands
                    .get(command)
                    .map(|command| command.arguments.clone())
            })
            .flatten()
    }
    pub fn require_capability(
        &self,
        name: &str,
        capability: Capability,
    ) -> Result<(), ServiceError> {
        self.policy(name)
            .ok_or_else(|| ServiceError::new(ErrorCode::Cancelled, "plugin is no longer loaded"))?
            .require(capability)
    }
    pub fn can_read(&self, name: &str) -> bool {
        self.require_capability(name, Capability::EditorRead)
            .is_ok()
    }
    pub fn subscribes(&self, event: Event) -> bool {
        self.plugins.values().any(|plugin| plugin.subscribes(event))
    }
    pub fn event_recipients(&self, event: Event) -> Vec<String> {
        self.plugins
            .iter()
            .filter(|(_, plugin)| plugin.subscribes(event))
            .map(|(name, _)| name.clone())
            .collect()
    }
    pub fn receives_event(&self, name: &str, event: Event) -> bool {
        self.plugins
            .get(name)
            .is_some_and(|plugin| plugin.subscribes(event))
    }
    pub fn call_event(
        &self,
        name: &str,
        event: Event,
        editor: EditorContext,
        data: Value,
        services: Arc<dyn HostServices>,
    ) -> Result<Completion, ServiceError> {
        let target = InvocationTarget::from_context(&editor);
        self.call_event_with_target(name, event, editor, data, services, target)
    }
    pub fn call_event_with_target(
        &self,
        name: &str,
        event: Event,
        editor: EditorContext,
        data: Value,
        services: Arc<dyn HostServices>,
        target: InvocationTarget,
    ) -> Result<Completion, ServiceError> {
        let plugin = self
            .plugins
            .get(name)
            .ok_or_else(|| ServiceError::new(ErrorCode::Cancelled, "plugin is no longer loaded"))?;
        if !plugin.subscribes(event) {
            return Err(ServiceError::new(
                ErrorCode::InvalidRequest,
                "plugin is not subscribed to event",
            ));
        }
        plugin.call(
            Request {
                abi_version: ABI_VERSION,
                event,
                command: None,
                args: Vec::new(),
                config: Value::Null,
                editor,
                data,
            },
            services,
            target,
        )
    }
    pub fn call_command(
        &self,
        name: &str,
        args: Vec<String>,
        editor: EditorContext,
        services: Arc<dyn HostServices>,
    ) -> Result<Option<Completion>, ServiceError> {
        self.call_command_with_data(name, args, editor, Value::Null, services)
    }
    pub fn call_command_with_data(
        &self,
        name: &str,
        args: Vec<String>,
        editor: EditorContext,
        data: Value,
        services: Arc<dyn HostServices>,
    ) -> Result<Option<Completion>, ServiceError> {
        let target = InvocationTarget::from_context(&editor);
        self.call_command_with_target(name, args, editor, data, services, target)
    }
    pub fn call_command_with_target(
        &self,
        name: &str,
        args: Vec<String>,
        editor: EditorContext,
        data: Value,
        services: Arc<dyn HostServices>,
        target: InvocationTarget,
    ) -> Result<Option<Completion>, ServiceError> {
        let Some((plugin_name, command)) = name.split_once('.') else {
            return Ok(None);
        };
        let Some(plugin) = self.plugins.get(plugin_name) else {
            return Ok(None);
        };
        let Some(details) = plugin.manifest.commands.get(command) else {
            return Ok(None);
        };
        if args.len() < details.arguments.min || args.len() > details.arguments.max {
            return Err(ServiceError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "command '{name}' expects {}..={} arguments, got {}",
                    details.arguments.min,
                    details.arguments.max,
                    args.len()
                ),
            ));
        }
        plugin
            .call(
                Request {
                    abi_version: ABI_VERSION,
                    event: Event::Command,
                    command: Some(command.into()),
                    args,
                    config: Value::Null,
                    editor,
                    data,
                },
                services,
                target,
            )
            .map(Some)
    }
    pub fn job_target(&self, name: &str, job: u64) -> Option<InvocationTarget> {
        self.plugins.get(name)?.actor.as_ref()?.job_target(job)
    }
    pub fn cancel_target(&self, document: Option<u64>, view: Option<u64>) {
        for plugin in self.plugins.values() {
            if let Some(actor) = &plugin.actor {
                actor.cancel_target(document, view);
            }
        }
    }
    /// Deliberate navigation may retire its own source while returning a reply.
    /// Only the initiating actor's active call is exempt; jobs and queued calls
    /// bound to the retired target are always cancelled.
    pub fn cancel_target_except(
        &self,
        document: Option<u64>,
        view: Option<u64>,
        plugin: &str,
        sequence: u64,
    ) {
        for (name, entry) in &self.plugins {
            if let Some(actor) = &entry.actor {
                actor.cancel_target_except(document, view, (name == plugin).then_some(sequence));
            }
        }
    }
    pub fn revoke(&self) {
        self.revoked.cancel();
        for plugin in self.plugins.values() {
            plugin.policy.revoke();
            if let Some(actor) = &plugin.actor {
                actor.revoke();
            }
        }
    }
    pub async fn shutdown(&self) -> Result<(), ServiceError> {
        self.revoke();
        let results = futures_util::future::join_all(
            self.plugins
                .values()
                .filter_map(|plugin| plugin.actor.as_ref())
                .map(|actor| actor.shutdown()),
        )
        .await;
        results
            .into_iter()
            .find_map(Result::err)
            .map_or(Ok(()), Err)
    }
}
impl Drop for PluginManager {
    fn drop(&mut self) {
        self.revoke();
    }
}
impl PluginSystem for PluginManager {
    fn available_commands(&self) -> Vec<PluginCommand> {
        self.available_commands()
    }
    fn get_doc_for_identifier(&self, identifier: &str) -> Option<String> {
        self.get_doc_for_identifier(identifier)
    }
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), ServiceError> {
    if cancel.is_cancelled() {
        Err(ServiceError::new(
            ErrorCode::Cancelled,
            "plugin package preparation cancelled",
        ))
    } else {
        Ok(())
    }
}
fn prepare_packages(
    configs: BTreeMap<String, PluginConfig>,
    base: PathBuf,
    generation: u64,
    mut pool: Option<WorkerPool>,
    preparing: Arc<AtomicBool>,
    cancel: CancellationToken,
) -> Result<PreparedManager, ServiceError> {
    if configs.values().filter(|config| config.enabled).count() > MAX_PLUGINS {
        return Err(ServiceError::new(
            ErrorCode::ResourceExhausted,
            format!("too many enabled plugins (limit {MAX_PLUGINS})"),
        ));
    }
    let mut packages = BTreeMap::new();
    let mut source_bytes = 0usize;
    for (name, config) in configs {
        if !config.enabled {
            continue;
        }
        check_cancel(&cancel)?;
        let package = Package::load(&name, &config, &base).map_err(|cause| {
            ServiceError::new(
                ErrorCode::InvalidRequest,
                format!("plugin '{name}': {cause:#}"),
            )
        })?;
        source_bytes =
            source_bytes.saturating_add(package.bytes.as_ref().map_or(0, |bytes| bytes.len()));
        if source_bytes > MAX_PACKAGE_SET_BYTES {
            return Err(ServiceError::new(
                ErrorCode::ResourceExhausted,
                "plugin replacement source exceeds 64 MiB",
            ));
        }
        packages.insert(
            name,
            PreparedPackage {
                package,
                instance: None,
            },
        );
    }
    check_cancel(&cancel)?;
    if packages
        .values()
        .any(|package| package.package.bytes.is_some())
        && pool.is_none()
    {
        pool = Some(WorkerPool::new()?);
    }
    let mut prepared = PreparedManager {
        packages,
        pool,
        preparing,
        generation,
    };
    for (name, package) in &mut prepared.packages {
        let Some(bytes) = package.package.bytes.take() else {
            continue;
        };
        check_cancel(&cancel)?;
        let preparation = prepared.pool.as_ref().unwrap().prepare(
            bytes,
            generation,
            package.package.policy.declared.clone(),
            package.package.policy.permissions.capabilities.clone(),
        )?;
        package.instance = Some(futures_executor::block_on(async {
            tokio::select! { _ = cancel.cancelled() => Err(ServiceError::new(ErrorCode::Cancelled, "plugin package preparation cancelled")), result = preparation => result }
        }).map_err(|error| ServiceError::new(error.code, format!("plugin '{name}': {}", error.message)))?);
    }
    check_cancel(&cancel)?;
    // An all-disabled/declarative replacement need not retain the old executor.
    if prepared
        .packages
        .values()
        .all(|package| package.instance.is_none())
    {
        prepared.pool = None;
    }
    Ok(prepared)
}

struct Package {
    manifest: Manifest,
    config: Arc<str>,
    policy: Arc<policy::AccessPolicy>,
    bytes: Option<Arc<[u8]>>,
}
impl Package {
    fn load(name: &str, config: &PluginConfig, base: &Path) -> Result<Self> {
        ensure!(
            valid_identifier(name),
            "invalid plugin name (use letters, digits, '-' or '_')"
        );
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
        let bytes = if let Some(module) = &manifest.module {
            ensure!(
                !module.as_os_str().is_empty()
                    && module
                        .components()
                        .all(|part| matches!(part, Component::Normal(_))),
                "module must be a relative path within the plugin directory"
            );
            let bytes = package.read_bounded(module, MAX_MODULE_BYTES)?;
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
            manifest,
            config: prepared_config,
            policy,
            bytes,
        })
    }
}

struct Plugin {
    manifest: Manifest,
    config: Arc<str>,
    policy: Arc<policy::AccessPolicy>,
    actor: Option<worker::PluginActor>,
}
impl Drop for Plugin {
    fn drop(&mut self) {
        self.policy.revoke();
        if let Some(actor) = &self.actor {
            actor.revoke();
        }
    }
}
impl Plugin {
    fn active(&self) -> bool {
        self.actor.as_ref().is_some_and(|actor| actor.is_active())
            && self.policy.check_live().is_ok()
    }
    fn subscribes(&self, event: Event) -> bool {
        self.active()
            && (!observes_editor_state(event)
                || self.policy.require(Capability::EditorRead).is_ok())
            && (matches!(
                event,
                Event::Init
                    | Event::Shutdown
                    | Event::ResyncRequired
                    | Event::State
                    | Event::UiResult
                    | Event::BuiltinResult
                    | Event::KeymapResult
                    | Event::JobReady
            ) || self.manifest.events.contains(&event))
    }
    fn call(
        &self,
        mut request: Request,
        services: Arc<dyn HostServices>,
        target: InvocationTarget,
    ) -> Result<Completion, ServiceError> {
        self.policy.check_live()?;
        if self.policy.require(Capability::EditorRead).is_err() {
            request.editor.document = None;
            request.editor.view = None;
            if !matches!(
                request.event,
                Event::Command
                    | Event::ResyncRequired
                    | Event::UiResult
                    | Event::BuiltinResult
                    | Event::KeymapResult
                    | Event::JobReady
            ) {
                request.data = Value::Null;
            }
        }
        self.actor
            .as_ref()
            .ok_or_else(|| {
                ServiceError::new(
                    ErrorCode::UnsupportedInterface,
                    "package has no executable component",
                )
            })?
            .invoke_configured(request, services, target, Some(self.config.clone()))
    }
}
fn valid_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests;
