//! Shared services run through the public view API without a terminal or compositor.
mod support;
use support::Fixture;

#[path = "handler_setup/clipboard.rs"]
mod clipboard;
#[path = "handler_setup/command_expansion.rs"]
mod command_expansion;
#[path = "handler_setup/completion.rs"]
mod completion;
#[path = "handler_setup/configuration.rs"]
mod configuration;
#[path = "handler_setup/debugger.rs"]
mod debugger;
#[path = "handler_setup/file_navigation.rs"]
mod file_navigation;
#[path = "handler_setup/file_watching.rs"]
mod file_watching;
#[path = "handler_setup/server_navigation.rs"]
mod server_navigation;
#[path = "handler_setup/snippets.rs"]
mod snippets;
#[path = "handler_setup/word_completion.rs"]
mod word_completion;
#[path = "handler_setup/workspace_changes.rs"]
mod workspace_changes;
#[path = "handler_setup/workspace_edits.rs"]
mod workspace_edits;
#[path = "handler_setup/workspace_resources.rs"]
mod workspace_resources;
#[path = "handler_setup/workspace_search.rs"]
mod workspace_search;
#[path = "handler_setup/workspace_trust.rs"]
mod workspace_trust;

#[path = "handler_setup/auto_save.rs"]
mod auto_save;
#[path = "handler_setup/diagnostic_refresh.rs"]
mod diagnostic_refresh;

#[cfg(feature = "git")]
#[path = "handler_setup/blame.rs"]
mod blame;

#[path = "handler_setup/document_lifecycle.rs"]
mod document_lifecycle;

#[path = "handler_setup/document_mutation.rs"]
mod document_mutation;

#[path = "handler_setup/editor_signals.rs"]
mod editor_signals;

#[path = "handler_setup/debugger_breakpoints.rs"]
mod debugger_breakpoints;
#[path = "handler_setup/document_transactions.rs"]
mod document_transactions;

#[path = "handler_setup/save.rs"]
mod save;

#[path = "handler_setup/formatting.rs"]
mod formatting;

#[path = "handler_setup/save_paths.rs"]
mod save_paths;
#[path = "handler_setup/write_completion.rs"]
mod write_completion;

#[path = "handler_setup/mode.rs"]
mod mode;

#[path = "handler_setup/document_settings.rs"]
mod document_settings;

#[cfg(unix)]
#[path = "handler_setup/shell.rs"]
mod shell;

#[path = "handler_setup/syntax_symbols.rs"]
mod syntax_symbols;

#[path = "handler_setup/buffer_search.rs"]
mod buffer_search;
