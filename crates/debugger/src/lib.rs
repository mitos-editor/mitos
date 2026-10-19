//! Debugger models independent of editor documents, callbacks, and terminal UI.
//!
//! Sessions own execution and adapter-specific breakpoint state. Source breakpoint
//! definitions outlive individual sessions. The editor owns asynchronous work,
//! trust decisions, and navigation, and applies their results to these models.

mod breakpoints;
mod execution;
mod lifecycle;
mod sessions;
mod source;
pub use source::{SourceId, SourceIds};

pub use breakpoints::{
    validate_capabilities, Breakpoint, BreakpointBinding, BreakpointBindings, BreakpointId,
    BreakpointLocation, BreakpointProperty, BreakpointSource, BreakpointStore, SourceBreakpoints,
};
pub use execution::{ExecutionState, StackFrames, StopTransition, ThreadState, ThreadStates};
pub use sessions::Sessions;

pub use lifecycle::SessionLifecycle;
