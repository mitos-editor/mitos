//! Compatibility exports for typable metadata, completion, and shared save options.

pub use super::catalog::{
    CommandCompleter, TypableCommand, SHELL_COMPLETER, SHELL_SIGNATURE, TYPABLE_COMMAND_LIST,
    TYPABLE_COMMAND_MAP,
};
pub use super::command_line::complete_command_args;
pub use super::files::typed::{write_all_impl, MoveBufferOptions, WriteAllOptions, WriteOptions};
