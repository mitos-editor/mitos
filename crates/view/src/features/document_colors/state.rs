//! Document color annotations and their request lifetime.

use editor_core::{Assoc, ChangeSet};
use event::TaskController;
use text_layout::text_annotations::InlineAnnotation;

use crate::Document;

#[derive(Default)]
pub(crate) struct DocumentColors {
    pub(super) cache: Option<DocumentColorSwatches>,
    pub(super) request: TaskController,
}

impl DocumentColors {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.request.cancel();
        self.clear_cache();
    }

    /// Keep cached anchors consistent before document-change observers run.
    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
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
    pub(super) fn clear_cache(&mut self) {
        self.cache = None;
    }
}

#[derive(Debug, Clone, Default)]
pub struct DocumentColorSwatches {
    pub color_swatches: Vec<InlineAnnotation>,
    pub colors: Vec<editor_core::highlight::Highlight>,
    pub color_swatches_padding: Vec<InlineAnnotation>,
}

impl Document {
    /// Cached LSP color annotations for rendering.
    pub fn color_swatches(&self) -> Option<&DocumentColorSwatches> {
        self.document_colors.cache.as_ref()
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn requests_outlive_views_and_cache_clears_but_not_the_document() {
        crate::document::check_document_request_lifetime(
            |doc| &mut doc.document_colors.request,
            |doc| doc.document_colors.clear_cache(),
        );
    }
}
