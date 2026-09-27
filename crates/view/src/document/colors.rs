//! Document color annotations and their request lifetime.

use editor_core::{syntax, text_annotations::InlineAnnotation};
use event::TaskController;

use super::Document;
use crate::handlers::document_colors::DocumentColorsHandler;

#[derive(Default)]
pub(crate) struct DocumentColors {
    pub(crate) cache: Option<DocumentColorSwatches>,
    pub(crate) request: TaskController,
    pub(crate) handler: Option<DocumentColorsHandler>,
}

impl DocumentColors {
    /// Clear rendered annotations without interrupting a replacement request.
    pub(crate) fn clear_cache(&mut self) {
        self.cache = None;
    }
}

#[derive(Debug, Clone, Default)]
pub struct DocumentColorSwatches {
    pub color_swatches: Vec<InlineAnnotation>,
    pub colors: Vec<syntax::Highlight>,
    pub color_swatches_padding: Vec<InlineAnnotation>,
}

impl Document {
    /// Cached LSP color annotations for rendering.
    pub fn color_swatches(&self) -> Option<&DocumentColorSwatches> {
        self.document_colors.cache.as_ref()
    }
}
