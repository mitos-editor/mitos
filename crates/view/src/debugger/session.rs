//! One editor-owned adapter lifetime, including pending startup and execution state.

use super::{
    breakpoints::BreakpointSession, events::SessionEvents, operations::SessionRequests,
    progress::ProgressState, reverse_requests::ReverseRequests, startup::Startup, DebuggerState,
};
use dap::{Client, DebugAdapterId, StackFrame, ThreadId};
use std::collections::HashMap;

/// Associates debugger thread identifiers with their editor display state.
pub type ThreadStates = HashMap<ThreadId, ThreadState>;

/// Execution state is separate from the adapter's free-form stop reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Running,
    Stopped(String),
}

impl ThreadState {
    pub fn is_stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Stopped(reason) => reason,
        }
    }
}

/// One adapter lifetime. Frontends inspect execution state; the editor owns its transitions.
pub struct DebugSession {
    id: DebugAdapterId,
    number: u64,
    pub(super) name: String,
    pub(super) parent: Option<DebugAdapterId>,
    pub(super) client: Option<Client>,
    pub(super) startup: Option<Startup>,
    pub(super) breakpoints: Option<BreakpointSession>,
    pub(super) requests: SessionRequests,
    pub(super) reverse_requests: ReverseRequests,
    pub(super) events: SessionEvents,
    pub(super) stack_frames: HashMap<ThreadId, Vec<StackFrame>>,
    pub(super) thread_states: ThreadStates,
    pub(super) thread_id: Option<ThreadId>,
    pub(super) active_frame: Option<usize>,
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
            startup: None,
            breakpoints: None,
            requests: SessionRequests::default(),
            reverse_requests: ReverseRequests::default(),
            events: SessionEvents::default(),
            stack_frames: HashMap::new(),
            thread_states: HashMap::new(),
            thread_id: None,
            active_frame: None,
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
        if self.events.is_terminating() {
            DebugSessionStatus::Disconnecting
        } else if self.startup.is_some()
            || self
                .breakpoints
                .as_ref()
                .is_some_and(|state| state.is_configuring())
        {
            DebugSessionStatus::Starting
        } else if self.thread_states.values().any(ThreadState::is_stopped) {
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
        self.client.as_ref()?.caps.as_ref()
    }

    pub fn connection_type(&self) -> Option<dap::ConnectionType> {
        self.client.as_ref()?.connection_type()
    }

    pub fn selected_thread(&self) -> Option<ThreadId> {
        self.thread_id
    }

    pub fn stack_frames(&self) -> &HashMap<ThreadId, Vec<StackFrame>> {
        &self.stack_frames
    }

    pub fn thread_states(&self) -> &ThreadStates {
        &self.thread_states
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.stack_frames
            .get(&self.thread_id?)?
            .get(self.active_frame?)
    }

    pub(super) fn resume_thread(&mut self, thread: ThreadId) {
        self.thread_states.insert(thread, ThreadState::Running);
        self.stack_frames.remove(&thread);
        if self.thread_id == Some(thread) {
            self.active_frame = None;
            self.thread_id = None;
        }
    }

    pub(super) fn resume_all_threads(&mut self) {
        for state in self.thread_states.values_mut() {
            *state = ThreadState::Running;
        }
        self.stack_frames.clear();
        self.active_frame = None;
        self.thread_id = None;
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
        self.get_session(*self.selection_order.last()?)
    }

    pub(crate) fn get_client(&self, id: DebugAdapterId) -> Option<&Client> {
        self.get_session(id)?.client.as_ref()
    }

    pub(crate) fn get_client_mut(&mut self, id: DebugAdapterId) -> Option<&mut Client> {
        self.get_session_mut(id)?.client.as_mut()
    }

    pub(crate) fn get_active_client(&self) -> Option<&Client> {
        let client = self.get_client(*self.selection_order.last()?)?;
        client.caps.as_ref()?;
        Some(client)
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.get_active_session()?.current_stack_frame()
    }

    /// Sessions in most recently selected order, including pending connections.
    pub fn sessions(&self) -> impl ExactSizeIterator<Item = &DebugSession> {
        self.selection_order
            .iter()
            .rev()
            .map(|id| &self.sessions[*id])
    }

    /// Select an existing session. Stale IDs cannot select a replacement session.
    pub fn select_session(&mut self, id: DebugAdapterId) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.sessions.contains_key(id),
            "Debugging session has ended"
        );
        if self.selection_order.last() == Some(&id) {
            return Ok(());
        }
        self.selection_order.retain(|session| *session != id);
        self.selection_order.push(id);
        self.current_selection = Some(self.selection.restart());
        Ok(())
    }

    pub(super) fn insert_session(
        &mut self,
        name: String,
        parent: Option<DebugAdapterId>,
    ) -> DebugAdapterId {
        self.next_session_number = self
            .next_session_number
            .checked_add(1)
            .expect("debug session numbers exhausted");
        let number = self.next_session_number;
        let id = self
            .sessions
            .insert_with_key(|id| DebugSession::new(id, number, name, parent));
        if parent.is_none() || self.selection_order.is_empty() {
            self.selection_order.push(id);
            self.current_selection = Some(self.selection.restart());
        } else {
            // Adapter-created child sessions do not take selection from the user.
            self.selection_order.insert(0, id);
        }
        id
    }

    /// Includes pending connections, even before a client can be selected.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn is_starting(&self, id: DebugAdapterId) -> bool {
        self.get_session(id)
            .is_some_and(|session| session.startup.is_some())
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
        self.sessions.remove(id);
        if self.selection_order.last() == Some(&id) {
            self.current_selection = Some(self.selection.restart());
        }
        self.selection_order.retain(|session| *session != id);
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
