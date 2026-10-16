//! Editor-owned debugger sessions and source breakpoints.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::bail;
use dap::{
    registry::{DebugAdapterId, Registry},
    Client, DebuggerCapabilities, StackFrame,
};
use editor_core::syntax::config::DebugAdapterConfig;
use lsp_client::block_on;

/// Editor-owned identity, retained across adapter IDs and source-line relocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BreakpointId(u64);

#[derive(Debug, Clone, Default)]
pub struct Breakpoint {
    pub(crate) identity: BreakpointId,
    pub id: Option<usize>,
    pub verified: bool,
    pub message: Option<String>,

    pub line: usize,
    pub column: Option<usize>,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

impl Breakpoint {
    pub fn identity(&self) -> BreakpointId {
        self.identity
    }

    fn set_property(
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

/// Owns debugger sessions and the source breakpoints shared across those sessions.
#[derive(Default)]
pub struct DebuggerState {
    pub(crate) adapters: Registry,
    pub(crate) breakpoints: HashMap<PathBuf, Vec<Breakpoint>>,
    next_breakpoint_id: u64,
}

impl DebuggerState {
    pub fn get_client(&self, id: DebugAdapterId) -> Option<&Client> {
        self.adapters.get_client(id)
    }

    pub fn get_client_mut(&mut self, id: DebugAdapterId) -> Option<&mut Client> {
        self.adapters.get_client_mut(id)
    }

    pub fn get_active_client(&self) -> Option<&Client> {
        self.adapters.get_active_client()
    }

    pub fn get_active_client_mut(&mut self) -> Option<&mut Client> {
        self.adapters.get_active_client_mut()
    }

    pub fn start_client(
        &mut self,
        socket: Option<SocketAddr>,
        config: &DebugAdapterConfig,
    ) -> dap::Result<DebugAdapterId> {
        self.adapters.start_client(socket, config)
    }

    pub fn unset_active_client(&mut self) {
        self.adapters.unset_active_client();
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.adapters.current_stack_frame()
    }

    /// Source breakpoints in a file, including breakpoints configured before debugging starts.
    pub fn breakpoints(&self, path: &Path) -> &[Breakpoint] {
        self.breakpoints
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn breakpoint_at_line(&self, path: &Path, line: usize) -> Option<&Breakpoint> {
        self.breakpoints(path)
            .iter()
            .find(|breakpoint| breakpoint.line == line)
    }

    /// Toggle each distinct source line once, then synchronize the active adapter if present.
    pub fn toggle_breakpoints(&mut self, path: PathBuf, lines: &[usize]) -> anyhow::Result<()> {
        let breakpoints = self.breakpoints.entry(path.clone()).or_default();
        let mut lines = lines.to_vec();
        lines.sort_unstable();
        lines.dedup();
        for line in lines {
            if let Some(pos) = breakpoints
                .iter()
                .position(|breakpoint| breakpoint.line == line)
            {
                breakpoints.remove(pos);
            } else {
                let identity = allocate_identity(&mut self.next_breakpoint_id);
                breakpoints.push(Breakpoint {
                    identity,
                    line,
                    ..Default::default()
                });
            }
        }
        self.sync_breakpoints(&path)
    }

    pub(crate) fn insert_breakpoint(&mut self, path: PathBuf, mut breakpoint: Breakpoint) {
        breakpoint.identity = allocate_identity(&mut self.next_breakpoint_id);
        self.breakpoints.entry(path).or_default().push(breakpoint);
    }

    /// Edit the original breakpoint, even if its adapter ID or source line changed.
    /// Removed identities cannot target a replacement breakpoint at the same line.
    pub fn set_breakpoint_property(
        &mut self,
        path: &Path,
        identity: BreakpointId,
        property: BreakpointProperty,
        value: Option<String>,
    ) -> anyhow::Result<()> {
        let breakpoints = self
            .breakpoints
            .get_mut(path)
            .ok_or_else(|| anyhow::anyhow!("Breakpoint no longer exists"))?;
        let position = breakpoints
            .iter()
            .position(|breakpoint| breakpoint.identity == identity)
            .ok_or_else(|| anyhow::anyhow!("Breakpoint no longer exists"))?;
        let capabilities = self
            .adapters
            .get_active_client()
            .and_then(|client| client.caps.as_ref());
        breakpoints[position].set_property(property, value, capabilities)?;
        self.sync_breakpoints(path)
    }

    fn sync_breakpoints(&mut self, path: &Path) -> anyhow::Result<()> {
        let Some(client) = self.adapters.get_active_client_mut() else {
            return Ok(());
        };
        let breakpoints = self
            .breakpoints
            .get_mut(path)
            .expect("breakpoint file is initialized before synchronization");
        sync_source_breakpoints(client, path.to_path_buf(), breakpoints)
    }
}

fn allocate_identity(next: &mut u64) -> BreakpointId {
    *next = next
        .checked_add(1)
        .expect("breakpoint identities exhausted");
    BreakpointId(*next)
}

fn validate_capabilities(
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

pub(crate) fn sync_source_breakpoints(
    debugger: &mut dap::Client,
    path: PathBuf,
    breakpoints: &mut [Breakpoint],
) -> Result<(), anyhow::Error> {
    if let Some(capabilities) = debugger.caps.as_ref() {
        for breakpoint in breakpoints.iter() {
            validate_capabilities(
                capabilities,
                breakpoint.condition.is_some(),
                breakpoint.hit_condition.is_some(),
                breakpoint.log_message.is_some(),
            )?;
        }
    }
    let source_breakpoints = breakpoints
        .iter()
        .map(|breakpoint| dap::SourceBreakpoint {
            line: breakpoint.line + 1, // convert from 0-indexing to 1-indexing (TODO: could set debugger to 0-indexing on init)
            column: breakpoint.column,
            condition: breakpoint.condition.clone(),
            hit_condition: breakpoint.hit_condition.clone(),
            log_message: breakpoint.log_message.clone(),
        })
        .collect::<Vec<_>>();

    let request = debugger.set_breakpoints(path, source_breakpoints);
    match block_on(request) {
        Ok(Some(dap_breakpoints)) => {
            for (breakpoint, dap_breakpoint) in breakpoints.iter_mut().zip(dap_breakpoints) {
                breakpoint.id = dap_breakpoint.id;
                breakpoint.verified = dap_breakpoint.verified;
                breakpoint.message = dap_breakpoint.message;
                // TODO: handle breakpoint.message
                // TODO: verify source matches
                breakpoint.line = dap_breakpoint.line.unwrap_or(0).saturating_sub(1); // convert to 0-indexing
                                                                                      // TODO: no unwrap
                breakpoint.column = dap_breakpoint.column;
                // TODO: verify end_linef/col instruction reference, offset
            }
        }
        Err(e) => anyhow::bail!("Failed to set breakpoints: {}", e),
        _ => {}
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_can_be_configured_before_a_debugger_starts() {
        let mut state = DebuggerState::default();
        let path = PathBuf::from("main.rs");
        state.toggle_breakpoints(path.clone(), &[2, 2, 4]).unwrap();
        let identity = state.breakpoint_at_line(&path, 4).unwrap().identity();
        state
            .set_breakpoint_property(
                &path,
                identity,
                BreakpointProperty::Condition,
                Some("ready".into()),
            )
            .unwrap();
        state.toggle_breakpoints(path.clone(), &[2]).unwrap();
        assert_eq!(state.breakpoints(&path).len(), 1);
        let breakpoint = state.breakpoint_at_line(&path, 4).unwrap();
        assert_eq!(breakpoint.condition.as_deref(), Some("ready"));
    }

    #[test]
    fn editing_a_removed_breakpoint_does_not_change_another_line() {
        let mut state = DebuggerState::default();
        let path = PathBuf::from("main.rs");
        state.toggle_breakpoints(path.clone(), &[2, 4]).unwrap();
        let identity = state.breakpoint_at_line(&path, 2).unwrap().identity();
        state.toggle_breakpoints(path.clone(), &[2]).unwrap();
        assert!(state
            .set_breakpoint_property(
                &path,
                identity,
                BreakpointProperty::Condition,
                Some("ready".into())
            )
            .is_err());
        assert!(state
            .breakpoint_at_line(&path, 4)
            .unwrap()
            .condition
            .is_none());
    }

    #[test]
    fn removed_breakpoint_identity_cannot_edit_a_replacement_at_the_same_line() {
        let mut state = DebuggerState::default();
        let path = PathBuf::from("main.rs");
        state.toggle_breakpoints(path.clone(), &[2]).unwrap();
        let identity = state.breakpoint_at_line(&path, 2).unwrap().identity();
        state.toggle_breakpoints(path.clone(), &[2]).unwrap();
        state.toggle_breakpoints(path.clone(), &[2]).unwrap();
        assert!(state
            .set_breakpoint_property(
                &path,
                identity,
                BreakpointProperty::Condition,
                Some("stale".into())
            )
            .is_err());
        let replacement = state.breakpoint_at_line(&path, 2).unwrap();
        assert_ne!(replacement.identity(), identity);
        assert!(replacement.condition.is_none());
    }

    #[test]
    fn breakpoint_identity_survives_adapter_line_and_id_changes() {
        let mut state = DebuggerState::default();
        let path = PathBuf::from("main.rs");
        state.toggle_breakpoints(path.clone(), &[2, 4]).unwrap();
        let identity = state.breakpoint_at_line(&path, 2).unwrap().identity();
        // Adapter verification may relocate a breakpoint and replace its session ID.
        let breakpoint = state
            .breakpoints
            .get_mut(&path)
            .unwrap()
            .first_mut()
            .unwrap();
        breakpoint.line = 8;
        breakpoint.id = Some(42);
        state
            .set_breakpoint_property(
                &path,
                identity,
                BreakpointProperty::Condition,
                Some("ready".into()),
            )
            .unwrap();
        assert_eq!(
            state
                .breakpoint_at_line(&path, 8)
                .unwrap()
                .condition
                .as_deref(),
            Some("ready")
        );
        assert!(state
            .breakpoint_at_line(&path, 4)
            .unwrap()
            .condition
            .is_none());
    }

    #[test]
    fn unsupported_breakpoint_properties_preserve_the_previous_configuration() {
        let mut breakpoint = Breakpoint::default();
        let mut capabilities: DebuggerCapabilities =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(breakpoint
            .set_property(
                BreakpointProperty::Condition,
                Some("ready".into()),
                Some(&capabilities)
            )
            .is_err());
        assert!(breakpoint.condition.is_none());
        capabilities.supports_conditional_breakpoints = Some(true);
        breakpoint
            .set_property(
                BreakpointProperty::Condition,
                Some("ready".into()),
                Some(&capabilities),
            )
            .unwrap();
        assert!(breakpoint
            .set_property(
                BreakpointProperty::LogMessage,
                Some("value".into()),
                Some(&capabilities)
            )
            .is_err());
        assert_eq!(breakpoint.condition.as_deref(), Some("ready"));
        assert!(breakpoint.log_message.is_none());
        // Clearing a setting remains valid when the current adapter cannot support it.
        capabilities.supports_conditional_breakpoints = Some(false);
        breakpoint
            .set_property(BreakpointProperty::Condition, None, Some(&capabilities))
            .unwrap();
        assert!(breakpoint.condition.is_none());
    }

    #[test]
    fn unsupported_properties_can_be_cleared_independently() {
        let mut breakpoint = Breakpoint {
            condition: Some("ready".into()),
            log_message: Some("value".into()),
            ..Default::default()
        };
        let capabilities: DebuggerCapabilities =
            serde_json::from_value(serde_json::json!({})).unwrap();
        breakpoint
            .set_property(BreakpointProperty::Condition, None, Some(&capabilities))
            .unwrap();
        assert!(breakpoint.condition.is_none());
        assert_eq!(breakpoint.log_message.as_deref(), Some("value"));
        breakpoint
            .set_property(BreakpointProperty::LogMessage, None, Some(&capabilities))
            .unwrap();
        assert!(breakpoint.log_message.is_none());
    }
}
