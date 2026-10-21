//! Document colors state, request coordination, and lifecycle hooks.

mod requests;
mod state;

pub use state::DocumentColorSwatches;
pub(crate) use state::DocumentColors;

pub(crate) use requests::request_document_colors;
