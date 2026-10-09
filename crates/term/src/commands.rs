//! Terminal command facade. Implementations and metadata live in their owning modules.

mod application;
mod buffers;
mod catalog;
mod command_line;
mod completion;
mod config;
mod context;
pub(crate) mod dap;
mod diagnostics;
mod editing;
mod files;
mod formatting;
mod history;
pub mod insert;
pub(crate) mod lsp;
mod macros;
mod mappable;
mod mode;
mod movement;
mod navigation;
mod palette;
mod picker;
mod plugins;
mod quicklist;
mod registers;
mod search;
mod selection;
pub(crate) mod shell;
mod snippets;
mod spelling;
mod symbols;
pub(crate) mod syntax;
mod textobjects;
mod vcs;
mod windows;
mod workspace;

pub use catalog::{
    CommandCompleter, TypableCommand, SHELL_COMPLETER, SHELL_SIGNATURE, TYPABLE_COMMAND_LIST,
    TYPABLE_COMMAND_MAP,
};
pub use command_line::complete_command_args;
pub use completion::completion;
pub(crate) use context::{CommandCompletion, CommandToken};
pub use context::{Context, OnKeyCallback, OnKeyCallbackKind};
pub use dap::{
    dap_continue, dap_disable_exceptions, dap_edit_condition, dap_edit_log, dap_enable_exceptions,
    dap_launch, dap_next, dap_pause, dap_restart, dap_start_impl, dap_step_in, dap_step_out,
    dap_switch_stack_frame, dap_switch_thread, dap_terminate, dap_toggle_breakpoints,
    dap_toggle_breakpoints_impl, dap_variables,
};
pub use diagnostics::{diagnostics_picker, workspace_diagnostics_picker};
pub(crate) use editing::replace_selections;
pub use files::typed::{write_all_impl, MoveBufferOptions};
pub use insert::{CommentContinuation, Open};
pub use lsp::{
    code_action, code_actions_on_save, compute_inlay_hints_for_all_views, goto_declaration,
    goto_definition, goto_implementation, goto_reference, goto_type_definition, hover,
    rename_symbol, select_references_to_symbol_under_cursor, signature_help, symbol_picker,
    workspace_symbol_picker,
};
pub use mappable::MappableCommand;
pub use movement::scroll;
pub use palette::command_palette;
pub(crate) use registers::{
    paste, paste_bracketed_value, replace_selections_with_register,
    yank_main_selection_to_register, Paste,
};
pub use syntax::{
    extend_parent_node_end, extend_parent_node_start, move_parent_node_end, move_parent_node_start,
    syntax_symbol_picker, syntax_workspace_symbol_picker,
};
pub use view::save::{WriteAllOptions, WriteOptions};
