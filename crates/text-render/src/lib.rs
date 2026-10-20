//! Paint formatted text and anchored decorations into Ratatui buffers.
//!
//! Callers supply text, layout, resolved highlight styles, and rendering settings.
//! Editor state, syntax parsing, feature-specific decorations, and terminal backends
//! remain with the application.

mod decorations;
mod renderer;

pub use decorations::{Decoration, DecorationManager};
pub use renderer::{render_text, LinePos, RenderSettings, TextRenderer};
