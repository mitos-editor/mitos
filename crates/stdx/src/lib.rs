//! Focused extensions to the Rust standard library used throughout Mitos.
//!
//! Keep helpers here independent of editor state and UI concerns. In
//! particular, [`path`] contains lexical and filesystem path handling,
//! [`rope`] adds operations whose units are explicit for `ropey` text, and
//! [`uri`] provides the editor's URL/URI representation and file-path
//! conversions.

pub mod env;
pub mod faccess;
pub mod file;
pub mod path;
pub mod range;
pub mod rope;
pub mod uri;

pub use range::Range;
pub use uri::Url;
