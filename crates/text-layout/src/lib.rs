//! Renderer-independent layout of document text.
//!
//! Converts text into display rows and cell columns, accounting for tabs, soft
//! wrapping, inline text, overlays, and line annotations. Buffer mutation and
//! document-line coordinates belong to `core`; this crate owns their display
//! interpretation and cursor movement through it.

pub mod doc_formatter;
pub mod movement;
mod position;
pub mod text_annotations;

pub use position::{
    char_idx_at_visual_block_offset, char_idx_at_visual_offset, softwrapped_dimensions,
    visual_offset_from_anchor, visual_offset_from_block, VisualOffsetError,
};
