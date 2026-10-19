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
    ) -> Self {
        Self {
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
pub struct BreakpointLocation<'a> {
    pub definition: Option<&'a Breakpoint>,
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
                .insert(id, BreakpointBinding::new(path, line, None, response));
        }
    }
}

/// Source definitions persist across adapter sessions; IDs are never reused.
#[derive(Default)]
pub struct BreakpointStore {
    definitions: HashMap<PathBuf, Vec<Breakpoint>>,
    next_identity: u64,
}

impl BreakpointStore {
    pub fn breakpoints(&self, path: &Path) -> &[Breakpoint] {
        self.definitions
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.definitions.keys()
    }
    pub fn insert(&mut self, path: PathBuf, line: usize) -> BreakpointId {
        self.next_identity = self
            .next_identity
            .checked_add(1)
            .expect("breakpoint identities exhausted");
        let identity = BreakpointId(self.next_identity);
        self.definitions.entry(path).or_default().push(Breakpoint {
            identity,
            line,
            ..Default::default()
        });
        identity
    }
    pub fn remove(&mut self, path: &Path, identity: BreakpointId) {
        if let Some(definitions) = self.definitions.get_mut(path) {
            definitions.retain(|definition| definition.identity != identity);
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
        let definition = self
            .definitions
            .get_mut(path)
            .and_then(|definitions| {
                definitions
                    .iter_mut()
                    .find(|definition| definition.identity == identity)
            })
            .ok_or_else(|| anyhow::anyhow!("Breakpoint no longer exists"))?;
        definition.set_property(property, value, capabilities)
    }
    /// Markers for a source file, including breakpoints discovered by the active adapter.
    pub fn locations<'a>(
        &'a self,
        path: &'a Path,
        session: Option<&'a BreakpointBindings>,
    ) -> impl Iterator<Item = BreakpointLocation<'a>> {
        self.breakpoints(path)
            .iter()
            .map(move |definition| {
                let binding =
                    session.and_then(|session| session.bindings.get(&definition.identity));
                BreakpointLocation {
                    definition: Some(definition),
                    line: binding.map_or(definition.line, |binding| binding.line),
                    verified: binding.is_some_and(|binding| binding.verified),
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
mod tests {
    use super::*;

    fn response(id: usize, line: Option<usize>) -> dap_types::Breakpoint {
        serde_json::from_value(serde_json::json!({"id":id,"verified":true,"line":line})).unwrap()
    }

    #[test]
    fn adapter_locations_do_not_change_requested_source_definitions() {
        let definitions = [Breakpoint {
            identity: BreakpointId(1),
            line: 7,
            ..Default::default()
        }];
        let mut session = BreakpointBindings::default();
        session.apply_response(
            Path::new("main.rs"),
            &definitions,
            vec![response(100, Some(42))],
        );
        assert_eq!(definitions[0].line, 7);
        assert_eq!(session.bindings[&BreakpointId(1)].line, 41);
        session.apply_response(
            Path::new("main.rs"),
            &definitions,
            vec![response(200, None)],
        );
        assert_eq!(session.bindings[&BreakpointId(1)].line, 7);
    }

    #[test]
    fn unknown_adapter_locations_preserve_requested_breakpoints() {
        let path = PathBuf::from("main.rs");
        let mut binding = BreakpointBinding::new(path.clone(), 7, Some(3), response(1, Some(0)));
        assert_eq!(binding.line, 7);
        binding.update(response(1, Some(0)));
        assert_eq!(binding.line, 7);
        let mut session = BreakpointBindings::default();
        let mut unknown = response(2, Some(0));
        unknown.source = Some(serde_json::from_value(serde_json::json!({"path":path})).unwrap());
        session.handle_event("new", unknown);
        assert!(session.discovered.is_empty());
    }
}
