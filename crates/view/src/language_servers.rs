//! Language-server integration with editor state and operations.

pub mod edits;
mod registry;
pub use registry::{has_required_root, language_server_root, Registry};
