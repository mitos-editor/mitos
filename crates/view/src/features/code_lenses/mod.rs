//! Code lenses state, request coordination, and lifecycle hooks.

mod requests;
mod state;

pub(crate) use requests::refresh_document;
pub(crate) use requests::register_hooks;
pub use requests::{
    prepare_visible_code_lenses, refresh_for_language_server, resolve_code_lenses, CodeLensCommand,
    DocumentCodeLensesHandler, ResolvedCodeLenses,
};
pub use state::CodeLens;
pub(crate) use state::DocumentCodeLenses;
