//! Parsing, elaboration, rendering, and interactive editing of LSP snippets.
//!
//! Snippets move through three representations:
//!
//! 1. [`Snippet::parse`] accepts LSP snippet syntax;
//! 2. [`Snippet`] elaborates duplicate and nested tabstops into a normalized
//!    model, always adding the final `$0` stop when absent; and
//! 3. [`Snippet::render`] creates document edits plus a [`RenderedSnippet`]
//!    whose ranges are tracked by [`ActiveSnippet`] as the document changes.
//!
//! [`TabstopIdx`] values are indices into the normalized tabstop vector, not the
//! numeric labels written in snippet source. In particular, source `$0` is
//! represented by [`LAST_TABSTOP_IDX`] during elaboration and becomes the last
//! normalized tabstop.

mod active;
mod elaborate;
mod parser;
mod render;

#[derive(PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Clone, Copy)]
/// Index of a tabstop in an elaborated or rendered snippet.
///
/// This is intentionally opaque because source labels are renumbered during
/// elaboration.
pub struct TabstopIdx(usize);
/// Sentinel used for the source-level `$0` final cursor position.
pub const LAST_TABSTOP_IDX: TabstopIdx = TabstopIdx(usize::MAX);

pub use active::ActiveSnippet;
pub use elaborate::{Snippet, SnippetElement, Transform};
pub use render::RenderedSnippet;
pub use render::SnippetRenderCtx;
