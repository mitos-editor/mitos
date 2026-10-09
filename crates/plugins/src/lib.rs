//! Sandboxed WebAssembly plugins and their command/lifecycle registry.
//!
//! Each configured plugin has its own instance and persistent memory. Guests have
//! no imports: editor access is restricted to the snapshots and actions defined
//! by `plugin-sdk`, rather than exposing editor internals or ambient I/O.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use plugin_sdk::{EditorContext, Event, Request, Response, ABI_VERSION};
use serde::Deserialize;
use serde_json::Value;
use wasmi::{
    Config, EnforcedLimits, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
    TypedFunc,
};

/// Maximum size of either JSON message crossing the plugin boundary.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MODULE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_ACTIONS: usize = 256;
const FUEL_PER_CALL: u64 = 10_000_000;

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
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Manifest {
    abi_version: u32,
    module: PathBuf,
    #[serde(default)]
    commands: BTreeMap<String, Command>,
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    doc: String,
}

/// A command registered by a plugin, qualified by its configured name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    pub name: String,
    pub doc: String,
}

/// The command registry boundary carried over from Helix's plugin proposal.
pub trait PluginSystem {
    fn available_commands(&self) -> Vec<PluginCommand>;
    fn get_doc_for_identifier(&self, identifier: &str) -> Option<String>;
}

/// Plugin state belonging to one editor, with independent failure isolation.
#[derive(Default)]
pub struct PluginManager {
    plugins: BTreeMap<String, Plugin>,
}

impl PluginManager {
    /// Load enabled plugins, collecting failures without discarding healthy plugins.
    pub fn load(configs: &BTreeMap<String, PluginConfig>, base: &Path) -> (Self, Vec<String>) {
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .enforced_limits(EnforcedLimits::strict())
            .ignore_custom_sections(true)
            .wasm_multi_memory(false)
            .set_max_recursion_depth(128)
            .set_max_stack_height(1024 * 1024);
        let engine = Engine::new(&config);
        let mut manager = Self::default();
        let mut errors = Vec::new();
        for (name, config) in configs {
            if !config.enabled {
                continue;
            }
            match Plugin::load(&engine, name, config, base) {
                Ok(plugin) => {
                    manager.plugins.insert(name.clone(), plugin);
                }
                Err(error) => errors.push(format!("plugin '{name}': {error:#}")),
            }
        }
        (manager, errors)
    }

    pub fn available_commands(&self) -> Vec<PluginCommand> {
        self.plugins
            .iter()
            .flat_map(|(name, plugin)| {
                plugin
                    .manifest
                    .commands
                    .iter()
                    .map(move |(command, details)| PluginCommand {
                        name: format!("{name}.{command}"),
                        doc: details.doc.clone(),
                    })
            })
            .collect()
    }

    pub fn get_doc_for_identifier(&self, identifier: &str) -> Option<String> {
        let (name, command) = identifier.split_once('.')?;
        self.plugins
            .get(name)?
            .manifest
            .commands
            .get(command)
            .map(|command| command.doc.clone())
    }

    /// Avoid constructing document snapshots when no active plugin needs an event.
    pub fn subscribes(&self, event: Event) -> bool {
        self.plugins.values().any(|plugin| plugin.subscribes(event))
    }

    /// Active recipients in deterministic configured-name order.
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
        &mut self,
        name: &str,
        event: Event,
        editor: EditorContext,
        data: Value,
    ) -> Result<Response> {
        let plugin = self
            .plugins
            .get_mut(name)
            .context("plugin is no longer loaded")?;
        ensure!(
            plugin.subscribes(event),
            "plugin is not subscribed to event"
        );
        plugin.call(&Request {
            abi_version: ABI_VERSION,
            event,
            command: None,
            args: Vec::new(),
            config: plugin.config.clone(),
            editor,
            data,
        })
    }

    /// Run a qualified command. Unknown commands are left to the editor's registry.
    pub fn call_command(
        &mut self,
        name: &str,
        args: Vec<String>,
        editor: EditorContext,
    ) -> Result<Option<Response>> {
        self.call_command_with_data(name, args, editor, Value::Null)
    }

    pub fn call_command_with_data(
        &mut self,
        name: &str,
        args: Vec<String>,
        editor: EditorContext,
        data: Value,
    ) -> Result<Option<Response>> {
        let Some((plugin_name, command)) = name.split_once('.') else {
            return Ok(None);
        };
        let Some(plugin) = self.plugins.get_mut(plugin_name) else {
            return Ok(None);
        };
        if !plugin.manifest.commands.contains_key(command) {
            return Ok(None);
        }
        let request = Request {
            abi_version: ABI_VERSION,
            event: Event::Command,
            command: Some(command.to_owned()),
            args,
            config: plugin.config.clone(),
            editor,
            data,
        };
        plugin
            .call(&request)
            .map(Some)
            .with_context(|| format!("plugin '{plugin_name}' command '{command}'"))
    }

    /// Dispatch lifecycle and subscribed editor events in configured-name order.
    pub fn dispatch_event(
        &mut self,
        event: Event,
        editor: EditorContext,
        data: Value,
    ) -> Vec<(String, Result<Response>)> {
        self.plugins
            .iter_mut()
            .filter(|(_, plugin)| plugin.subscribes(event))
            .map(|(name, plugin)| {
                let request = Request {
                    abi_version: ABI_VERSION,
                    event,
                    command: None,
                    args: Vec::new(),
                    config: plugin.config.clone(),
                    editor: editor.clone(),
                    data: data.clone(),
                };
                (name.clone(), plugin.call(&request))
            })
            .collect()
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

struct Plugin {
    manifest: Manifest,
    config: Value,
    instance: Option<Guest>,
}

impl Plugin {
    fn load(engine: &Engine, name: &str, config: &PluginConfig, base: &Path) -> Result<Self> {
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
        let manifest_bytes = read_bounded(&manifest_path, MAX_MANIFEST_BYTES)?;
        let manifest: Manifest = toml::from_str(std::str::from_utf8(&manifest_bytes)?)
            .context("invalid plugin manifest")?;
        ensure!(
            manifest.abi_version == ABI_VERSION,
            "unsupported ABI version {} (expected {ABI_VERSION})",
            manifest.abi_version
        );
        ensure!(
            manifest.commands.keys().all(|name| valid_identifier(name)),
            "invalid command name (use letters, digits, '-' or '_')"
        );
        ensure!(
            !manifest.module.as_os_str().is_empty()
                && manifest
                    .module
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "module must be a relative path within the plugin directory"
        );
        let directory = manifest_path
            .parent()
            .context("manifest has no parent directory")?;
        let module_path = directory
            .join(&manifest.module)
            .canonicalize()
            .with_context(|| format!("opening module {}", manifest.module.display()))?;
        ensure!(
            module_path.starts_with(directory),
            "module must stay within the plugin directory"
        );
        let bytes = read_bounded(&module_path, MAX_MODULE_BYTES)?;
        // Reject text-format modules even when Wasmi's optional WAT feature is enabled elsewhere.
        ensure!(
            bytes.starts_with(b"\0asm"),
            "module must be a WebAssembly binary"
        );
        let module = Module::new(engine, &bytes).context("invalid WebAssembly module")?;
        ensure!(
            module.imports().next().is_none(),
            "plugin modules must not import host or WASI functions"
        );
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY_BYTES)
            .memories(1)
            .table_elements(4096)
            .tables(1)
            .instances(1)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(FUEL_PER_CALL)?;
        let instance = Linker::<StoreLimits>::new(engine)
            .instantiate_and_start(&mut store, &module)
            .context("instantiating WebAssembly plugin")?;
        let memory = instance
            .get_memory(&store, "memory")
            .context("missing 'memory' export")?;
        let alloc = instance
            .get_typed_func(&store, "mitos_alloc")
            .context("missing or invalid 'mitos_alloc' export")?;
        let dealloc = instance
            .get_typed_func(&store, "mitos_dealloc")
            .context("missing or invalid 'mitos_dealloc' export")?;
        let call = instance
            .get_typed_func(&store, "mitos_call")
            .context("missing or invalid 'mitos_call' export")?;
        Ok(Self {
            manifest,
            config: config.config.clone(),
            instance: Some(Guest {
                store,
                memory,
                alloc,
                dealloc,
                call,
            }),
        })
    }

    fn subscribes(&self, event: Event) -> bool {
        self.instance.is_some()
            && (matches!(
                event,
                Event::Init | Event::Shutdown | Event::ResyncRequired | Event::State
            ) || self.manifest.events.contains(&event))
    }

    fn call(&mut self, request: &Request) -> Result<Response> {
        ensure!(
            self.instance.is_some(),
            "disabled after a previous failure; use :plugin-reload to reload"
        );
        // A host snapshot that is too large is not a guest failure.
        let mut bytes = BoundedBytes::default();
        serde_json::to_writer(&mut bytes, request)
            .context("serializing plugin request (limit 4 MiB)")?;
        let result = self.instance.as_mut().unwrap().call(&bytes.0);
        if result.is_err() {
            // Discard the entire instance, including allocations left behind by a trap.
            self.instance = None;
        }
        result
    }
}

struct Guest {
    store: Store<StoreLimits>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    dealloc: TypedFunc<(i32, i32), ()>,
    call: TypedFunc<(i32, i32), i64>,
}

impl Guest {
    fn call(&mut self, request: &[u8]) -> Result<Response> {
        self.store.set_fuel(FUEL_PER_CALL)?;
        let input_len = i32::try_from(request.len())?;
        let input_ptr = self
            .alloc
            .call(&mut self.store, input_len)
            .context("allocating guest request")?;
        let input_range = self.buffer_range(input_ptr as u32, request.len())?;
        self.memory
            .write(&mut self.store, input_range.start, request)
            .context("writing guest request")?;
        let output = self
            .call
            .call(&mut self.store, (input_ptr, input_len))
            .context("executing guest handler")? as u64;
        let output_ptr = (output >> 32) as u32;
        let output_len = (output & u32::MAX as u64) as usize;
        ensure!(
            output_len > 0 && output_len <= MAX_MESSAGE_BYTES,
            "invalid response length {output_len} (limit 4 MiB)"
        );
        let output_range = self.buffer_range(output_ptr, output_len)?;
        ensure!(
            input_range.end <= output_range.start || output_range.end <= input_range.start,
            "response buffer overlaps the borrowed request buffer"
        );
        let response: Response =
            serde_json::from_slice(&self.memory.data(&self.store)[output_range])
                .context("invalid plugin response JSON")?;
        ensure!(
            response.actions.len() <= MAX_ACTIONS,
            "too many plugin actions (limit {MAX_ACTIONS})"
        );
        self.dealloc
            .call(&mut self.store, (output_ptr as i32, output_len as i32))
            .context("freeing guest response")?;
        self.dealloc
            .call(&mut self.store, (input_ptr, input_len))
            .context("freeing guest request")?;
        Ok(response)
    }

    fn buffer_range(&self, pointer: u32, length: usize) -> Result<std::ops::Range<usize>> {
        let start = pointer as usize;
        let end = start
            .checked_add(length)
            .context("guest buffer range overflow")?;
        ensure!(
            end <= self.memory.data_size(&self.store),
            "guest buffer is outside linear memory"
        );
        Ok(start..end)
    }
}

fn valid_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        file.metadata()?.len() <= limit as u64,
        "{} exceeds the {} byte limit",
        path.display(),
        limit
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "{} exceeds the {} byte limit",
        path.display(),
        limit
    );
    Ok(bytes)
}

#[derive(Default)]
struct BoundedBytes(Vec<u8>);

impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_MESSAGE_BYTES - self.0.len() {
            return Err(io::Error::other("plugin request exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
