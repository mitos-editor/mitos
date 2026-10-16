//! Document highlight state, request coordination, and lifecycle hooks.

mod requests;
mod state;

pub(crate) use requests::register_hooks;
pub use requests::DocumentHighlightHandler;
pub use state::DocumentHighlights;
pub(crate) use state::DocumentHighlightsState;
