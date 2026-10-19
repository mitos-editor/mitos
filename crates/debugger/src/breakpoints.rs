//! Persistent source definitions and adapter-local bindings.
use anyhow::bail;
use dap_types::DebuggerCapabilities;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
fn line_index(line: usize) -> Option<usize> {
    line.checked_sub(1)
}

/// Stable identity retained across adapter sessions and source-line relocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct BreakpointId(u64);

#[derive(Debug, Clone, Default)]
pub struct Breakpoint {
    identity: BreakpointId,
    revision: u64,
    pub line: usize,
    /// Optional one-based UTF-16 column, matching the DAP source-breakpoint format.
    pub column: Option<usize>,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

impl Breakpoint {
    pub fn identity(&self) -> BreakpointId {
        self.identity
    }

    pub fn set_property(
        &mut self,
        property: BreakpointProperty,
        value: Option<String>,
        capabilities: Option<&DebuggerCapabilities>,
    ) -> anyhow::Result<()> {
        if value.is_some()
            && let Some(capabilities) = capabilities
        {
            // Validate newly requested requirements. Clearing a setting must
            // remain possible even if other stored settings are unsupported.
            validate_capabilities(
                capabilities,
                matches!(property, BreakpointProperty::Condition),
                false,
                matches!(property, BreakpointProperty::LogMessage),
            )?;
        }
        match property {
            BreakpointProperty::Condition => self.condition = value,
            BreakpointProperty::LogMessage => self.log_message = value,
        }
        self.revision = self
            .revision
            .checked_add(1)
            .expect("breakpoint revision exhausted");
        Ok(())
    }
}

/// Editable source-breakpoint properties, independent of the prompt that collected them.
#[derive(Clone, Copy)]
pub enum BreakpointProperty {
    Condition,
    LogMessage,
}

pub fn validate_capabilities(
    capabilities: &DebuggerCapabilities,
    conditional: bool,
    hit_conditional: bool,
    logpoint: bool,
) -> anyhow::Result<()> {
    if conditional
        && !capabilities
            .supports_conditional_breakpoints
            .unwrap_or_default()
    {
        bail!("Can't edit breakpoint: debugger does not support conditional breakpoints");
    }
    if hit_conditional
        && !capabilities
            .supports_hit_conditional_breakpoints
            .unwrap_or_default()
    {
        bail!("Can't edit breakpoint: debugger does not support hit conditional breakpoints");
    }
    if logpoint && !capabilities.supports_log_points.unwrap_or_default() {
        bail!("Can't edit breakpoint: debugger does not support logpoints");
    }
    Ok(())
}

/// Adapter-specific verification and location. Requested source definitions are unchanged.
#[derive(Debug, Clone)]
pub struct BreakpointBinding {
    requested_revision: u64,
    pub id: Option<usize>,
    pub verified: bool,
    pub message: Option<String>,
    pub path: PathBuf,
    pub line: usize,
    /// Optional one-based UTF-16 column supplied by the adapter.
    pub column: Option<usize>,
}

impl BreakpointBinding {
    fn new(
        path: PathBuf,
        line: usize,
        column: Option<usize>,
        response: dap_types::Breakpoint,
        requested_revision: u64,
    ) -> Self {
        Self {
            requested_revision,
            id: response.id,
            verified: response.verified,
            message: response.message,
            path,
            line: response.line.and_then(line_index).unwrap_or(line),
            column: response.column.filter(|column| *column > 0).or(column),
        }
    }

    fn update(&mut self, response: dap_types::Breakpoint) {
        self.verified = response.verified;
        self.message = response.message;
        if let Some(line) = response.line.and_then(line_index) {
            self.line = line;
        }
        if let Some(column) = response.column.filter(|column| *column > 0) {
            self.column = Some(column);
        }
    }
}

/// A source marker projected through the active adapter's bindings, or its requested location.
pub struct BreakpointLocation {
    pub definition: Option<Breakpoint>,
    pub line: usize,
    pub verified: bool,
}

#[derive(Default)]
pub struct BreakpointBindings {
    bindings: HashMap<BreakpointId, BreakpointBinding>,
    discovered: HashMap<usize, BreakpointBinding>,
}

impl BreakpointBindings {
    pub fn get(&self, identity: BreakpointId) -> Option<&BreakpointBinding> {
        self.bindings.get(&identity)
    }
    pub fn get_for(&self, definition: &Breakpoint) -> Option<&BreakpointBinding> {
        self.get(definition.identity())
            .filter(|binding| binding.requested_revision == definition.revision)
    }
    pub fn remove_binding(&mut self, identity: BreakpointId) {
        self.bindings.remove(&identity);
    }

    pub fn apply_response(
        &mut self,
        path: &Path,
        definitions: &[Breakpoint],
        response: Vec<dap_types::Breakpoint>,
    ) {
        self.bindings.retain(|_, binding| binding.path != path);
        for (definition, response) in definitions.iter().zip(response) {
            if let Some(id) = response.id {
                self.discovered.remove(&id);
            }
            self.bindings.insert(
                definition.identity,
                BreakpointBinding::new(
                    path.to_owned(),
                    definition.line,
                    definition.column,
                    response,
                    definition.revision,
                ),
            );
        }
    }

    pub fn handle_event(&mut self, reason: &str, response: dap_types::Breakpoint) {
        let Some(id) = response.id else {
            log::warn!("DAP breakpoint event missing id");
            return;
        };
        if reason == "removed" {
            self.bindings.retain(|_, binding| binding.id != Some(id));
            self.discovered.remove(&id);
            return;
        }
        if reason != "new" && reason != "changed" {
            log::warn!("Unknown breakpoint event: {reason}");
            return;
        }
        if let Some(binding) = self
            .bindings
            .values_mut()
            .find(|binding| binding.id == Some(id))
            .or_else(|| self.discovered.get_mut(&id))
        {
            binding.update(response);
            return;
        }
        // Breakpoints created by the adapter belong only to this session.
        if reason == "new"
            && let Some(path) = response
                .source
                .as_ref()
                .and_then(|source| source.path.clone())
            && let Some(line) = response.line.and_then(line_index)
        {
            self.discovered
                .insert(id, BreakpointBinding::new(path, line, None, response, 0));
        }
    }
}

/// The editor and an open document share one source's definitions. Queries return
/// snapshots so no lock is held while applying editor effects or adapter replies.
#[derive(Clone, Default)]
pub struct SourceBreakpoints(std::sync::Arc<parking_lot::RwLock<Vec<Breakpoint>>>);

impl SourceBreakpoints {
    pub fn snapshot(&self) -> Vec<Breakpoint> {
        self.0.read().clone()
    }

    /// Move markers through a document edit. Deleted lines use the nearest surviving
    /// line; if markers collapse onto one line, retain the oldest identity.
    pub fn map_positions(&self, mut map: impl FnMut(&Breakpoint) -> (usize, Option<usize>)) {
        let mut definitions = self.0.write();
        let mut lines = std::collections::HashSet::new();
        for definition in definitions.iter_mut() {
            (definition.line, definition.column) = map(definition);
            definition.revision = definition
                .revision
                .checked_add(1)
                .expect("breakpoint revision exhausted");
        }
        definitions.retain(|definition| lines.insert(definition.line));
    }
}

/// Source definitions persist across adapter sessions; IDs are never reused.
#[derive(Default)]
pub struct BreakpointStore {
    definitions: HashMap<PathBuf, SourceBreakpoints>,
    next_identity: u64,
}

impl BreakpointStore {
    pub fn source(&mut self, path: PathBuf) -> SourceBreakpoints {
        self.definitions.entry(path).or_default().clone()
    }
    pub fn breakpoints(&self, path: &Path) -> Vec<Breakpoint> {
        self.definitions
            .get(path)
            .map(SourceBreakpoints::snapshot)
            .unwrap_or_default()
    }
    pub fn definition(&self, identity: BreakpointId) -> Option<Breakpoint> {
        self.definitions.values().find_map(|source| {
            source
                .0
                .read()
                .iter()
                .find(|definition| definition.identity == identity)
                .cloned()
        })
    }
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.definitions
            .iter()
            .filter(|(_, source)| !source.0.read().is_empty())
            .map(|(path, _)| path)
    }
    pub fn insert(&mut self, path: PathBuf, line: usize) -> BreakpointId {
        self.next_identity = self
            .next_identity
            .checked_add(1)
            .expect("breakpoint identities exhausted");
        let identity = BreakpointId(self.next_identity);
        self.source(path).0.write().push(Breakpoint {
            identity,
            line,
            ..Default::default()
        });
        identity
    }
    pub fn remove(&mut self, path: &Path, identity: BreakpointId) {
        if let Some(source) = self.definitions.get(path) {
            source
                .0
                .write()
                .retain(|definition| definition.identity != identity);
        }
    }
    pub fn set_property(
        &mut self,
        path: &Path,
        identity: BreakpointId,
        property: BreakpointProperty,
        value: Option<String>,
        capabilities: Option<&DebuggerCapabilities>,
    ) -> anyhow::Result<()> {
        let source = self
            .definitions
            .get(path)
            .ok_or_else(|| anyhow::anyhow!("Breakpoint no longer exists"))?;
        let mut definitions = source.0.write();
        definitions
            .iter_mut()
            .find(|definition| definition.identity == identity)
            .ok_or_else(|| anyhow::anyhow!("Breakpoint no longer exists"))?
            .set_property(property, value, capabilities)
    }
    /// Markers for a source file, including breakpoints discovered by the active adapter.
    pub fn locations<'a>(
        &self,
        path: &'a Path,
        session: Option<&'a BreakpointBindings>,
    ) -> impl Iterator<Item = BreakpointLocation> + 'a {
        self.breakpoints(path)
            .into_iter()
            .map(move |definition| {
                let binding = session.and_then(|session| session.get_for(&definition));
                BreakpointLocation {
                    line: binding.map_or(definition.line, |binding| binding.line),
                    verified: binding.is_some_and(|binding| binding.verified),
                    definition: Some(definition),
                }
            })
            .chain(
                session
                    .into_iter()
                    .flat_map(|session| session.discovered.values())
                    .filter(move |binding| binding.path == path)
                    .map(|binding| BreakpointLocation {
                        definition: None,
                        line: binding.line,
                        verified: binding.verified,
                    }),
            )
    }
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn mapped_definitions_reject_adapter_locations_from_older_revisions() {
        let path = PathBuf::from("main.rs");
        let mut store = BreakpointStore::default();
        let identity = store.insert(path.clone(), 2);
        let submitted = store.breakpoints(&path);
        store
            .source(path.clone())
            .map_positions(|breakpoint| (breakpoint.line + 1, breakpoint.column));
        let mut bindings = BreakpointBindings::default();
        bindings.apply_response(
            &path,
            &submitted,
            vec![
                serde_json::from_value(serde_json::json!({"id":1,"verified":true,"line":3}))
                    .unwrap(),
            ],
        );
        let marker = store.locations(&path, Some(&bindings)).next().unwrap();
        assert_eq!(marker.definition.unwrap().identity(), identity);
        assert_eq!(marker.line, 3);
        assert!(!marker.verified);
    }
}
