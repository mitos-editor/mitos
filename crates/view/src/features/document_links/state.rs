//! Document links and their request lifetime.

use editor_core::{Assoc, ChangeSet};
use event::TaskController;
use lsp_client::{lsp, LanguageServerId};

use super::DocumentLinksHandler;
use crate::Document;

#[derive(Default)]
pub(crate) struct DocumentLinks {
    pub(super) cache: Vec<DocumentLink>,
    pub(super) request: TaskController,
    pub(super) handler: Option<DocumentLinksHandler>,
}

impl DocumentLinks {
    pub(crate) fn bind_handler(&mut self, handler: DocumentLinksHandler) {
        self.handler = Some(handler);
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        changes.update_positions(self.cache.iter_mut().flat_map(|link| {
            std::iter::once((&mut link.start, Assoc::After))
                .chain(std::iter::once((&mut link.end, Assoc::After)))
        }));
    }

    /// Clear cached navigation targets without interrupting a replacement request.
    pub(super) fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

#[derive(Debug, Clone)]
pub struct DocumentLink {
    /// Character offsets in the document for the link range.
    pub start: usize,
    pub end: usize,
    pub link: lsp::DocumentLink,
    pub language_server_id: LanguageServerId,
}

impl Document {
    /// Cached LSP links for navigation and rendering, ordered by document position.
    pub fn document_links(&self) -> &[DocumentLink] {
        &self.document_links.cache
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
                editor_core::syntax::Loader::default(),
            )),
        );
        let view = crate::ViewId::default();
        doc.ensure_view_init(view);
        let request = doc.document_links.request.restart();
        doc.remove_view(view);
        assert!(!request.is_canceled());
        doc.document_links.clear_cache();
        assert!(!request.is_canceled());
        drop(doc);
        assert!(request.is_canceled());
    }
}
