//! Language definitions, syntax analysis, and structural editing.
//!
//! Text primitives belong to `core`. This crate adds parsers, language resources,
//! and operations that inspect syntax without depending on editor or frontend state.
pub mod indent;
pub mod loader;
pub mod match_brackets;
pub mod movement;
pub mod object;
pub mod queries;
pub mod surround;
pub mod syntax;
pub mod textobject;

pub use syntax::{Loader, LoaderError, Syntax};
pub use tree_house::{tree_sitter, Language};
