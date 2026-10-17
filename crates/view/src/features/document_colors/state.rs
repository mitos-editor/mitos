//! Document color annotations and their request lifetime.

use editor_core::{Assoc, ChangeSet};
use event::TaskController;
use text_layout::text_annotations::InlineAnnotation;

use super::DocumentColorsHandler;
use crate::Document;

#[derive(Default)]
pub(crate) struct DocumentColors {
    pub(super) cache: Option<DocumentColorSwatches>,
    pub(super) request: TaskController,
    pub(super) handler: Option<DocumentColorsHandler>,
}

impl DocumentColors {
    pub(crate) fn bind_handler(&mut self, handler: DocumentColorsHandler) {
        self.handler = Some(handler);
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
    use super::*;

    #[tokio::test]
    async fn requests_outlive_views_and_cache_clears_but_not_the_document() {
        let mut doc = Document::from(
            editor_core::Rope::from("sample text"),
            None,
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                crate::config::Config::default(),
            )),
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                language::syntax::Loader::default(),
            )),
        );
        let view = crate::ViewId::default();
        doc.ensure_view_init(view);
        let request = doc.document_colors.request.restart();
        doc.remove_view(view);
        assert!(!request.is_canceled());
        doc.document_colors.clear_cache();
        assert!(!request.is_canceled());
        drop(doc);
        assert!(request.is_canceled());
    }
}
