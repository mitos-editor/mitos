//! One editor-owned adapter lifetime, including pending startup and execution state.

use super::{
    breakpoints::BreakpointSession, events::SessionEvents, operations::SessionRequests,
    progress::ProgressState, startup::Startup, DebuggerState,
};
use dap::{Client, DebugAdapterId, StackFrame, ThreadId};
use std::collections::HashMap;

/// Associates debugger thread identifiers with their editor display state.
pub type ThreadStates = HashMap<ThreadId, String>;

/// One adapter lifetime. Frontends inspect execution state; the editor owns its transitions.
pub struct DebugSession {
    id: DebugAdapterId,
    pub(super) client: Option<Client>,
    pub(super) startup: Option<Startup>,
    pub(super) breakpoints: Option<BreakpointSession>,
    pub(super) requests: SessionRequests,
    pub(super) events: SessionEvents,
    pub(super) stack_frames: HashMap<ThreadId, Vec<StackFrame>>,
    pub(super) thread_states: ThreadStates,
    pub(super) thread_id: Option<ThreadId>,
    pub(super) active_frame: Option<usize>,
    pub(super) progress: HashMap<String, ProgressState>,
}

impl DebugSession {
    pub(super) fn new(id: DebugAdapterId) -> Self {
        Self {
            id,
            client: None,
            startup: None,
            breakpoints: None,
            requests: SessionRequests::default(),
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

    /// Clears stack-frame state after execution resumes.
    ///
    /// Command handlers should call this after a successful continue or step so
    /// paused-state UI is not rendered from stale frames.
    pub(super) fn resume_application(&mut self) {
        if let Some(thread_id) = self.thread_id {
            self.thread_states.insert(thread_id, "running".to_string());
            self.stack_frames.remove(&thread_id);
        }
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
        self.get_session(self.active_session?)
    }

    pub fn get_client(&self, id: DebugAdapterId) -> Option<&Client> {
        self.get_session(id)?.client.as_ref()
    }

    pub fn get_client_mut(&mut self, id: DebugAdapterId) -> Option<&mut Client> {
        self.get_session_mut(id)?.client.as_mut()
    }

    pub fn get_active_client(&self) -> Option<&Client> {
        self.get_client(self.active_session?)
    }

    pub fn get_active_client_mut(&mut self) -> Option<&mut Client> {
        self.get_client_mut(self.active_session?)
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.get_active_session()?.current_stack_frame()
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

    pub(crate) fn remove_session(&mut self, id: DebugAdapterId) {
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
        self.cancel_startup_requests(id);
        self.sessions.remove(id);
        if self.active_session == Some(id) {
            self.active_session = None;
        }
    }
}
