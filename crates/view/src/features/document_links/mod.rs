//! Document links state, request coordination, and lifecycle hooks.

mod navigation;
mod requests;
mod state;

pub(crate) use requests::register_hooks;
pub use requests::DocumentLinksHandler;
pub(crate) use state::DocumentLinks;
pub use state::{DocumentLink, ReferenceLink};

pub use navigation::{
    prepare_navigation, FileNavigation, FileNavigationTask, FileNavigationUpdate, NavigationTarget,
};
