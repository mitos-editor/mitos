//! Editor-owned debugger sessions and source breakpoints.

mod breakpoints;
pub mod source;
pub use ::debugger::{BreakpointBinding, BreakpointLocation};
mod events;
mod progress;
mod reverse_requests;
mod session;
pub use session::{DebugSession, DebugSessionStatus};
mod launch;
pub use launch::DebuggerLaunch;
mod startup;
pub(crate) use startup::register_hooks;
mod operations;
pub use operations::{
    DebuggerCommand, DebuggerContext, DebuggerFrames, DebuggerScope, DebuggerTask, DebuggerUpdate,
};

use std::path::{Path, PathBuf};

use ::debugger::validate_capabilities;
pub use ::debugger::{Breakpoint, BreakpointId, BreakpointProperty, ThreadState, ThreadStates};
use ::debugger::{BreakpointStore, Sessions};
use dap::DebugAdapterId;
#[cfg(test)]
use dap::DebuggerCapabilities;
use futures_util::stream::SelectAll;
use tokio_stream::wrappers::UnboundedReceiverStream;

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
    sessions: Sessions<DebugAdapterId, DebugSession>,
    selection: event::TaskController,
    current_selection: Option<event::TaskHandle>,
    pub(crate) incoming: SelectAll<UnboundedReceiverStream<(DebugAdapterId, dap::ClientEvent)>>,
    breakpoints: BreakpointStore,
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
        self.breakpoints.breakpoints(path)
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
            if let Some(identity) = existing {
                self.breakpoints.remove(&path, identity);
                for session in self
                    .sessions
                    .values_mut()
                    .filter_map(|session| session.breakpoints.as_mut())
                {
                    session.remove_binding(identity);
                }
            } else {
                self.breakpoints.insert(path.clone(), line);
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
        let capabilities = self.sessions.active().and_then(DebugSession::capabilities);
        self.breakpoints
            .set_property(path, identity, property, value, capabilities)?;
        Ok(())
    }
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
        let mut breakpoint = Breakpoint::default();
        breakpoint.condition = Some("ready".into());
        breakpoint.log_message = Some("value".into());
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
