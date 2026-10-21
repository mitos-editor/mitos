//! Document links state, request coordination, and lifecycle hooks.

mod navigation;
mod requests;
mod state;

pub(crate) use requests::register_hooks;
pub use state::DocumentLink;
pub(crate) use state::DocumentLinks;

pub use navigation::{
    prepare_navigation, FileNavigation, FileNavigationTask, FileNavigationUpdate, NavigationTarget,
};

pub(crate) use requests::request_document_links;
