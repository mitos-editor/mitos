//! Feature-owned state, requests, and lifecycle coordination.
//!
//! Documents own feature state; editor handlers own callback destinations.
//! Frontends trigger operations and consume validated updates or cached data.

pub mod auto_reload;
pub mod auto_save;
pub mod blame;
pub mod code_action_hint;
pub mod code_lenses;
pub mod completion;
pub(crate) mod debounce;
pub mod diagnostics;
pub mod document_colors;
pub mod document_highlight;
pub mod document_links;
pub mod document_symbols;
pub mod formatting;
pub mod hover;
pub mod inlay_hints;
pub mod inline_completion;
pub mod interactive;
pub mod navigation;
pub(crate) mod request;
pub mod signature_help;
pub mod spelling;
pub mod syntax;
pub mod word_index;
pub mod workspace_trust;
