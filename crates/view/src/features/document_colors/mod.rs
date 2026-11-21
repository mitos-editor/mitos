//! LSP document colors and their cached rendering annotations.

mod requests;
mod state;

pub(super) use requests::refresh;
pub use requests::DocumentColorsHandler;
pub use state::DocumentColorSwatches;
pub(crate) use state::DocumentColors;
