//! Document links state, request coordination, and lifecycle hooks.

mod requests;
mod state;

pub(crate) use requests::register_hooks;
pub use requests::DocumentLinksHandler;
pub use state::DocumentLink;
pub(crate) use state::DocumentLinks;
