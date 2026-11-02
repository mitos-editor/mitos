//! Document symbols state, request coordination, and lifecycle hooks.

mod picker;
pub use picker::{DocumentSymbol, DocumentSymbolsTask, DocumentSymbolsUpdate};
mod requests;
mod state;

pub(crate) use requests::register_hooks;
pub use requests::DocumentSymbolsHandler;
pub use state::Crumb;
pub(crate) use state::DocumentSymbols;
