//! Per-view code-action availability and request coordination.
mod requests;
mod state;
pub(crate) use requests::register_hooks;
pub use requests::CodeActionHintHandler;
pub(crate) use state::CodeActionHints;
