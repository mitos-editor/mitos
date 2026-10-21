//! Language-server integration with editor state and operations.

mod capabilities;
pub(crate) use capabilities::supports_feature;
pub mod edits;
mod registry;
pub(crate) use registry::Registry;
pub use registry::{has_required_root, language_server_root};
