//! Synchronous thread/frame transitions; callers own request cancellation.
use dap_types::{StackFrame, ThreadId};
use std::collections::HashMap;

/// Execution state indexed by adapter thread identifiers.
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

/// A loaded prefix of a thread's trace. Short replies finish even if the total is stale.
#[derive(Default)]
pub struct StackFrames {
    frames: Vec<StackFrame>,
    more: bool,
}

impl StackFrames {
    pub fn new(
        frames: Vec<StackFrame>,
        total: Option<usize>,
        start: usize,
        page_size: Option<usize>,
    ) -> Self {
        let more = page_size.is_some_and(|size| frames.len() == size)
            && total.is_none_or(|total| start.saturating_add(frames.len()) < total);
        Self { frames, more }
    }
    pub fn frames(&self) -> &[StackFrame] {
        &self.frames
    }
    /// Extend this prefix once; duplicate or out-of-order pages are rejected.
    pub fn append(&mut self, start: usize, page: StackFrames) -> bool {
        if self.frames.len() != start || !self.more {
            return false;
        }
        self.frames.extend(page.frames);
        self.more = page.more;
        true
    }

    pub fn has_more(&self) -> bool {
        self.more
    }
}

#[derive(Default)]
pub struct ExecutionState {
    stack_frames: HashMap<ThreadId, StackFrames>,
    thread_states: ThreadStates,
    thread_id: Option<ThreadId>,
    active_frame: Option<usize>,
}

pub struct StopTransition {
    pub previous_thread: Option<ThreadId>,
    pub needs_frames: bool,
}

impl ExecutionState {
    pub fn selected_thread(&self) -> Option<ThreadId> {
        self.thread_id
    }
    pub fn stack_frames(&self) -> &HashMap<ThreadId, StackFrames> {
        &self.stack_frames
    }
    pub fn thread_states(&self) -> &ThreadStates {
        &self.thread_states
    }
    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.stack_frames
            .get(&self.thread_id?)?
            .frames
            .get(self.active_frame?)
    }

    /// Invalidate stale frames as soon as a stop arrives, before fetching replacements.
    pub fn begin_stop(
        &mut self,
        thread: Option<ThreadId>,
        all: bool,
        reason: &str,
    ) -> StopTransition {
        let previous_thread = self.thread_id;
        let needs_frames = all
            || thread.is_none()
            || previous_thread == thread
            || self.current_stack_frame().is_none();
        self.thread_id = previous_thread.or(thread);
        if all {
            self.stack_frames.clear();
            for state in self.thread_states.values_mut() {
                *state = ThreadState::Stopped(reason.to_owned());
            }
        } else if let Some(thread) = thread {
            self.stack_frames.remove(&thread);
        }
        if needs_frames {
            self.active_frame = None;
        }
        if let Some(thread) = thread {
            self.thread_states
                .insert(thread, ThreadState::Stopped(reason.to_owned()));
        }
        StopTransition {
            previous_thread,
            needs_frames,
        }
    }

    pub fn apply_stopped_threads(
        &mut self,
        threads: &[ThreadId],
        selected: Option<ThreadId>,
        reason: &str,
        all: bool,
    ) {
        if all {
            self.thread_states
                .retain(|thread, _| threads.contains(thread));
        }
        for thread in threads {
            self.thread_states
                .insert(*thread, ThreadState::Stopped(reason.to_owned()));
        }
        self.thread_id = selected;
    }

    pub fn select_thread(&mut self, thread: ThreadId, frames: StackFrames) {
        self.thread_id = Some(thread);
        self.active_frame = (!frames.frames.is_empty()).then_some(0);
        self.stack_frames.insert(thread, frames);
    }

    /// Replace a refreshed trace while retaining the selected frame by adapter identity.
    pub fn refresh_stack_frames(&mut self, thread: ThreadId, frames: StackFrames) {
        let selected = self.current_stack_frame().map(|frame| frame.id);
        self.select_thread(thread, frames);
        if let Some(selected) = selected
            && let Some(index) = self.stack_frames[&thread]
                .frames
                .iter()
                .position(|frame| frame.id == selected)
        {
            self.active_frame = Some(index);
        }
    }

    pub fn select_frame(&mut self, index: usize) -> Option<&StackFrame> {
        let frames = self.stack_frames.get(&self.thread_id?)?;
        let frame = frames.frames.get(index)?;
        self.active_frame = Some(index);
        Some(frame)
    }

    /// Append only to the exact prefix for which the request was issued.
    pub fn append_stack_frames(
        &mut self,
        thread: ThreadId,
        start: usize,
        page: StackFrames,
    ) -> bool {
        let Some(trace) = self.stack_frames.get_mut(&thread) else {
            return false;
        };
        trace.append(start, page)
    }

    pub fn resume_thread(&mut self, thread: ThreadId) {
        self.thread_states.insert(thread, ThreadState::Running);
        self.stack_frames.remove(&thread);
        if self.thread_id == Some(thread) {
            self.active_frame = None;
            self.thread_id = None;
        }
    }

    pub fn resume_all_threads(&mut self) {
        for state in self.thread_states.values_mut() {
            *state = ThreadState::Running;
        }
        self.stack_frames.clear();
        self.active_frame = None;
        self.thread_id = None;
    }

    pub fn thread_started(&mut self, thread: ThreadId) {
        self.thread_states.insert(thread, ThreadState::Running);
    }

    pub fn remove_thread(&mut self, thread: ThreadId) {
        self.thread_states.remove(&thread);
        self.stack_frames.remove(&thread);
        if self.thread_id == Some(thread) {
            self.thread_id = None;
            self.active_frame = None;
        }
    }

    pub fn clear(&mut self) {
        self.resume_all_threads();
        self.thread_states.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_resume_preserves_other_stopped_threads() {
        let first = serde_json::from_str("1").unwrap();
        let second = serde_json::from_str("2").unwrap();
        let mut execution = ExecutionState::default();
        execution.begin_stop(Some(first), true, "breakpoint");
        execution.apply_stopped_threads(&[first, second], Some(first), "breakpoint", true);
        execution.resume_thread(second);
        assert!(execution.thread_states()[&first].is_stopped());
        assert_eq!(execution.thread_states()[&second], ThreadState::Running);
        assert_eq!(execution.selected_thread(), Some(first));
        execution.remove_thread(first);
        assert_eq!(execution.selected_thread(), None);
    }
}
