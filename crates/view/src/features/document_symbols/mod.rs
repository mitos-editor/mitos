//! Document symbols state, request coordination, and lifecycle hooks.

mod requests;
mod state;

pub(crate) use requests::register_hooks;
pub use requests::DocumentSymbolsHandler;
pub(crate) use state::DocumentSymbols;
pub use state::{Breadcrumbs, Crumb};
