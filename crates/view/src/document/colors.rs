//! Document color annotations and their request lifetime.

use editor_core::{syntax, text_annotations::InlineAnnotation, Assoc, ChangeSet};
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
    /// Keep cached anchors consistent before document-change observers run.
    pub(super) fn apply_changes(&mut self, changes: &ChangeSet) {
        if let Some(cache) = &mut self.cache {
            for annotations in [&mut cache.color_swatches, &mut cache.color_swatches_padding] {
                changes.update_positions(
                    annotations
                        .iter_mut()
                        .map(|annotation| (&mut annotation.char_idx, Assoc::After)),
                );
            }
        }
    }

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
