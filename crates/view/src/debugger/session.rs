//! One editor-owned adapter lifetime, including pending startup and execution state.

use super::{
    breakpoints::BreakpointSession, events::SessionEvents, operations::SessionRequests,
    progress::ProgressState, reverse_requests::ReverseRequests, startup::Startup, DebuggerState,
};
use ::debugger::{ExecutionState, SessionLifecycle, ThreadState, ThreadStates};
use dap::{Client, DebugAdapterId, StackFrame, ThreadId};
use std::collections::HashMap;

/// One adapter lifetime. Frontends inspect execution state; the editor owns its transitions.
pub struct DebugSession {
    id: DebugAdapterId,
    number: u64,
    pub(super) name: String,
    pub(super) parent: Option<DebugAdapterId>,
    pub(super) client: Option<Client>,
    pub(super) adapter: Option<language_config::DebugAdapterConfig>,
    pub(super) capabilities: Option<dap::DebuggerCapabilities>,
    pub(super) launch: Option<super::DebuggerLaunch>,
    pending_capabilities: Option<dap::DebuggerCapabilities>,
    pub(super) lifecycle: SessionLifecycle,
    pub(super) startup: Option<Startup>,
    pub(super) breakpoints: Option<BreakpointSession>,
    pub(super) requests: SessionRequests,
    pub(super) reverse_requests: ReverseRequests,
    pub(super) events: SessionEvents,
    pub(super) execution: ExecutionState,
    pub(super) progress: HashMap<String, ProgressState>,
}

/// Display state of a session, independent of frontend selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugSessionStatus {
    Starting,
    Running,
    Stopped,
    Disconnecting,
}

impl std::fmt::Display for DebugSessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Disconnecting => "disconnecting",
        })
    }
}

impl DebugSession {
    fn new(id: DebugAdapterId, number: u64, name: String, parent: Option<DebugAdapterId>) -> Self {
        Self {
            id,
            number,
            name,
            parent,
            client: None,
            adapter: None,
            capabilities: None,
            launch: None,
            pending_capabilities: None,
            lifecycle: SessionLifecycle::default(),
            startup: None,
            breakpoints: None,
            requests: SessionRequests::default(),
            reverse_requests: ReverseRequests::default(),
            events: SessionEvents::default(),
            execution: ExecutionState::default(),
            progress: HashMap::new(),
        }
    }

    pub fn id(&self) -> DebugAdapterId {
        self.id
    }

    /// A stable display number, unique for this editor lifetime.
    pub fn number(&self) -> u64 {
        self.number
    }

    pub fn status(&self) -> DebugSessionStatus {
        if self.lifecycle.is_disconnecting() {
            DebugSessionStatus::Disconnecting
        } else if self.lifecycle.is_starting() || self.lifecycle.is_configuring() {
            DebugSessionStatus::Starting
        } else if self
            .execution
            .thread_states()
            .values()
            .any(ThreadState::is_stopped)
        {
            DebugSessionStatus::Stopped
        } else {
            DebugSessionStatus::Running
        }
    }

    /// Adapter and launch-template label, available before the adapter connects.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn parent(&self) -> Option<DebugAdapterId> {
        self.parent
    }

    /// Whether the adapter transport has connected, including during initialization.
    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    /// Negotiated capabilities, including subsequent adapter updates.
    pub fn capabilities(&self) -> Option<&dap::DebuggerCapabilities> {
        self.capabilities.as_ref()
    }

    pub(crate) fn update_capabilities(&mut self, update: dap::DebuggerCapabilities) {
        if let Some(capabilities) = self.capabilities.as_mut() {
            capabilities.apply_update(update);
            return;
        }
        match &mut self.pending_capabilities {
            Some(capabilities) => capabilities.apply_update(update),
            None => self.pending_capabilities = Some(update),
        }
    }

    pub(super) fn initialize_capabilities(&mut self, mut capabilities: dap::DebuggerCapabilities) {
        if let Some(update) = self.pending_capabilities.take() {
            capabilities.apply_update(update);
        }
        self.capabilities = Some(capabilities);
    }

    pub fn connection_type(&self) -> Option<dap::ConnectionType> {
        self.launch.as_ref().map(|launch| launch.request)
    }

    pub fn selected_thread(&self) -> Option<ThreadId> {
        self.execution.selected_thread()
    }

    pub fn stack_frames(&self) -> &HashMap<ThreadId, Vec<StackFrame>> {
        self.execution.stack_frames()
    }

    pub fn thread_states(&self) -> &ThreadStates {
        self.execution.thread_states()
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.execution.current_stack_frame()
    }
    pub(super) fn resume_thread(&mut self, thread: ThreadId) {
        self.execution.resume_thread(thread);
    }
    pub(super) fn resume_all_threads(&mut self) {
        self.execution.resume_all_threads();
    }
}

impl DebuggerState {
    pub fn get_session(&self, id: DebugAdapterId) -> Option<&DebugSession> {
        self.sessions.get(id)
    }

    pub(crate) fn get_session_mut(&mut self, id: DebugAdapterId) -> Option<&mut DebugSession> {
        self.sessions.get_mut(id)
    }

    pub fn get_active_session(&self) -> Option<&DebugSession> {
        self.sessions.active()
    }

    pub(crate) fn get_client(&self, id: DebugAdapterId) -> Option<&Client> {
        self.get_session(id)?.client.as_ref()
    }

    pub(crate) fn get_active_client(&self) -> Option<&Client> {
        let session = self.get_active_session()?;
        session.capabilities()?;
        session.client.as_ref()
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.get_active_session()?.current_stack_frame()
    }

    /// Sessions in most recently selected order, including pending connections.
    pub fn sessions(&self) -> impl ExactSizeIterator<Item = &DebugSession> {
        self.sessions.by_selection()
    }

    /// Select an existing session. Stale IDs cannot select a replacement session.
    pub fn select_session(&mut self, id: DebugAdapterId) -> anyhow::Result<()> {
        if self.sessions.select(id)? {
            self.current_selection = Some(self.selection.restart());
        }
        Ok(())
    }

    pub(super) fn insert_session(
        &mut self,
        name: String,
        parent: Option<DebugAdapterId>,
    ) -> DebugAdapterId {
        let previous = self.sessions.active_id();
        let id = self.sessions.insert(parent.is_none(), |id, number| {
            DebugSession::new(id, number, name, parent)
        });
        if previous != self.sessions.active_id() {
            self.current_selection = Some(self.selection.restart());
        }
        id
    }

    /// Includes pending connections, even before a client can be selected.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn is_starting(&self, id: DebugAdapterId) -> bool {
        self.get_session(id)
            .is_some_and(|session| session.lifecycle.is_starting())
    }

    /// Cancel connection, initialization, or launch without affecting running sessions.
    pub fn cancel_startup(&mut self, id: DebugAdapterId) -> bool {
        if !self.is_starting(id) {
            return false;
        }
        self.remove_session(id);
        true
    }

    pub(super) fn cancel_child_startups(&mut self, id: DebugAdapterId) {
        let children: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(child, session)| {
                (session.startup.as_ref()?.parent() == Some(id)).then_some(child)
            })
            .collect();
        for child in children {
            self.remove_session(child);
        }
    }

    pub(crate) fn remove_session(&mut self, id: DebugAdapterId) {
        self.cancel_child_startups(id);
        self.cancel_startup_requests(id);
        let selected = self.sessions.active_id() == Some(id);
        self.sessions.remove(id);
        if selected {
            self.current_selection = Some(self.selection.restart());
        }
    }
}

impl crate::Editor {
    /// Select a session and navigate to its cached stopped frame, if available.
    pub fn select_debugger_session(&mut self, id: DebugAdapterId) -> anyhow::Result<()> {
        self.debugger.select_session(id)?;
        if let Some(frame) = self.debugger.current_stack_frame().cloned() {
            crate::handlers::dap::jump_to_stack_frame(self, &frame);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_sessions_restores_selection_history_and_rejects_stale_ids() {
        let mut state = DebuggerState::default();
        let first = state.insert_session("first".into(), None);
        let second = state.insert_session("second".into(), None);
        let third = state.insert_session("third".into(), None);
        state.select_session(first).unwrap();
        state.remove_session(second);
        assert_eq!(state.get_active_session().unwrap().id(), first);
        state.remove_session(first);
        assert_eq!(state.get_active_session().unwrap().id(), third);
        let replacement = state.insert_session("replacement".into(), None);
        assert!(state.select_session(first).is_err());
        assert_eq!(state.get_active_session().unwrap().id(), replacement);
        state.remove_session(replacement);
        state.remove_session(third);
        assert!(state.get_active_session().is_none());
        assert_eq!(state.sessions().len(), 0);
    }

    #[test]
    fn child_sessions_preserve_selection_and_remain_selectable() {
        let mut state = DebuggerState::default();
        let parent = state.insert_session("parent".into(), None);
        let child = state.insert_session("child".into(), Some(parent));
        assert_eq!(state.get_active_session().unwrap().id(), parent);
        assert_eq!(state.get_session(child).unwrap().parent(), Some(parent));
        state.select_session(child).unwrap();
        state.remove_session(child);
        assert_eq!(state.get_active_session().unwrap().id(), parent);
    }
}
