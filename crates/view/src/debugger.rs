//! Editor-owned debugger sessions and source breakpoints.

mod breakpoints;
pub use breakpoints::{BreakpointBinding, BreakpointLocation};
mod events;
mod progress;
mod session;
pub use session::{DebugSession, DebugSessionStatus, ThreadStates};
mod launch;
mod startup;
pub(crate) use startup::register_hooks;
mod operations;
pub use operations::{
    DebuggerCommand, DebuggerContext, DebuggerFrames, DebuggerScope, DebuggerTask, DebuggerUpdate,
};

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::bail;
use dap::{DebugAdapterId, DebuggerCapabilities};
use futures_util::stream::SelectAll;
use slotmap::SlotMap;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Editor-owned identity, retained across adapter IDs and source-line relocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct BreakpointId(u64);

#[derive(Debug, Clone, Default)]
pub struct Breakpoint {
    pub(crate) identity: BreakpointId,
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

/// Frontend-owned launch of a debuggee in a terminal. The shared editor owns
/// reverse-request completion; implementations honor working directory and environment.
pub trait TerminalLauncher: Send + Sync {
    fn launch(
        &self,
        args: dap::requests::RunInTerminalArguments,
    ) -> futures_util::future::BoxFuture<'static, dap::Result<dap::requests::RunInTerminalResponse>>;
}

/// Owns debugger sessions and the source breakpoints shared across those sessions.
#[derive(Default)]
pub struct DebuggerState {
    sessions: SlotMap<DebugAdapterId, DebugSession>,
    next_session_number: u64,
    // Least recently selected first; the last session is active, including during startup.
    selection_order: Vec<DebugAdapterId>,
    selection: event::TaskController,
    current_selection: Option<event::TaskHandle>,
    pub(crate) incoming: SelectAll<UnboundedReceiverStream<(DebugAdapterId, dap::Payload)>>,
    pub(crate) breakpoints: HashMap<PathBuf, Vec<Breakpoint>>,
    next_breakpoint_id: u64,
    terminal_launcher: Option<Box<dyn TerminalLauncher>>,
}

impl DebuggerState {
    /// Install a frontend terminal operation before starting any debug adapters.
    pub fn set_terminal_launcher(
        &mut self,
        launcher: impl TerminalLauncher + 'static,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.is_empty(),
            "Configure terminal launching before starting debug adapters"
        );
        self.terminal_launcher = Some(Box::new(launcher));
        Ok(())
    }

    pub(crate) fn run_in_terminal(
        &self,
        args: dap::requests::RunInTerminalArguments,
    ) -> futures_util::future::BoxFuture<'static, dap::Result<dap::requests::RunInTerminalResponse>>
    {
        match &self.terminal_launcher {
            Some(launcher) => launcher.launch(args),
            None => Box::pin(async {
                Err(anyhow::anyhow!("This frontend does not provide debug terminals").into())
            }),
        }
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
            .find(|breakpoint| self.breakpoint_line(breakpoint) == line)
    }

    /// Toggle each distinct source line once in the stored breakpoint model.
    pub(crate) fn toggle_breakpoints(&mut self, path: PathBuf, lines: &[usize]) {
        let mut lines = lines.to_vec();
        lines.sort_unstable();
        lines.dedup();
        for line in lines {
            let existing = self
                .breakpoint_at_line(&path, line)
                .map(Breakpoint::identity);
            let breakpoints = self.breakpoints.entry(path.clone()).or_default();
            if let Some(identity) = existing {
                breakpoints.retain(|breakpoint| breakpoint.identity != identity);
                for session in self
                    .sessions
                    .values_mut()
                    .filter_map(|session| session.breakpoints.as_mut())
                {
                    session.remove_binding(identity);
                }
            } else {
                let identity = allocate_identity(&mut self.next_breakpoint_id);
                breakpoints.push(Breakpoint {
                    identity,
                    line,
                    ..Default::default()
                });
            }
        }
    }

    /// Edit the original breakpoint, even if its adapter ID or source line changed.
    /// Removed identities cannot target a replacement breakpoint at the same line.
    pub(crate) fn set_breakpoint_property(
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
            .selection_order
            .last()
            .and_then(|id| self.sessions.get(*id))
            .and_then(|session| session.client.as_ref()?.caps.as_ref());
        breakpoints[position].set_property(property, value, capabilities)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_can_be_configured_before_a_debugger_starts() {
        let mut state = DebuggerState::default();
        let path = PathBuf::from("main.rs");
        state.toggle_breakpoints(path.clone(), &[2, 2, 4]);
        let identity = state.breakpoint_at_line(&path, 4).unwrap().identity();
        state
            .set_breakpoint_property(
                &path,
                identity,
                BreakpointProperty::Condition,
                Some("ready".into()),
            )
            .unwrap();
        state.toggle_breakpoints(path.clone(), &[2]);
        assert_eq!(state.breakpoints(&path).len(), 1);
        let breakpoint = state.breakpoint_at_line(&path, 4).unwrap();
        assert_eq!(breakpoint.condition.as_deref(), Some("ready"));
    }

    #[test]
    fn editing_a_removed_breakpoint_does_not_change_another_line() {
        let mut state = DebuggerState::default();
        let path = PathBuf::from("main.rs");
        state.toggle_breakpoints(path.clone(), &[2, 4]);
        let identity = state.breakpoint_at_line(&path, 2).unwrap().identity();
        state.toggle_breakpoints(path.clone(), &[2]);
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
        state.toggle_breakpoints(path.clone(), &[2]);
        let identity = state.breakpoint_at_line(&path, 2).unwrap().identity();
        state.toggle_breakpoints(path.clone(), &[2]);
        state.toggle_breakpoints(path.clone(), &[2]);
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
