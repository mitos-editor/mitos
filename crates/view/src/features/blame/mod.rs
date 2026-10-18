//! Git blame state, background requests, and lifecycle coordination.
mod details;
mod requests;
mod state;
pub use details::{next_update, request_details, BlameDetailsUpdate};
pub(crate) use requests::register_hooks;
pub use requests::BlameHandler;
pub(crate) use state::DocumentBlame;
pub use state::LineBlameError;
