//! Feature-owned state, requests, and lifecycle coordination.
//!
//! Documents own feature state; editor handlers own callback destinations.
//! Frontends trigger operations and consume validated updates or cached data.

pub mod code_lenses;
pub(crate) mod debounce;
pub mod document_colors;
pub mod document_highlight;
pub mod document_links;
pub mod document_symbols;
pub mod hover;
pub mod inlay_hints;
pub mod navigation;
pub(crate) mod request;
