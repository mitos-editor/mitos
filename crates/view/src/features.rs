//! Feature-owned state, requests, and lifecycle coordination.
//!
//! Documents own feature state; editor handlers own callback destinations.
//! Frontends trigger operations and consume validated updates or cached data.

pub mod auto_reload;
pub mod auto_save;
pub mod blame;
pub mod buffer_search;
pub mod change_review;
pub mod code_action_hint;
pub mod code_actions;
pub mod code_lenses;
pub mod completion;
pub(crate) mod debounce;
pub mod diagnostics;
pub mod document_colors;
pub mod document_highlight;
pub mod document_links;
pub mod document_symbols;
pub mod formatting;
#[cfg(feature = "git")]
pub mod git_operations;
pub mod hover;
pub mod inlay_hints;
pub mod inline_completion;
pub mod lsp_progress;
pub mod navigation;
pub(crate) mod process;
pub mod rename;
pub(crate) mod request;
pub mod shell;
pub mod signature_help;
pub mod spelling;
pub mod syntax;
pub mod syntax_symbols;
pub mod word_index;
pub mod workspace_changes;
pub mod workspace_symbols;
pub mod workspace_trust;

pub mod workspace_search;

pub mod snippets;
