//! Code lenses state, request coordination, and lifecycle hooks.

mod requests;
mod state;

pub(crate) use requests::refresh_document;
pub use requests::{
    prepare_visible_code_lenses, refresh_for_language_server, resolve_code_lenses, CodeLensCommand,
    ResolvedCodeLenses,
};
pub use state::CodeLens;
pub(crate) use state::DocumentCodeLenses;

pub(crate) use requests::request_document_code_lenses;
