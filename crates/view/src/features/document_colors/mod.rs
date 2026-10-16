//! Document colors state, request coordination, and lifecycle hooks.

mod requests;
mod state;

pub(crate) use requests::register_hooks;
pub use requests::DocumentColorsHandler;
pub use state::DocumentColorSwatches;
pub(crate) use state::DocumentColors;
