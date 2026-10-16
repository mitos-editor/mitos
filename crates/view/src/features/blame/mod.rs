//! Git blame state, background requests, and lifecycle coordination.
mod requests;
mod state;
pub(crate) use requests::register_hooks;
pub use requests::BlameHandler;
pub(crate) use state::DocumentBlame;
pub use state::LineBlameError;
