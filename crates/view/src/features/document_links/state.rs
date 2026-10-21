//! Document links and their request lifetime.

use editor_core::{Assoc, ChangeSet};
use event::{TaskController, TaskHandle};
use lsp_client::{lsp, LanguageServerId};

use super::DocumentLinksHandler;
use crate::{Document, ViewId};

#[derive(Default)]
pub(crate) struct DocumentLinks {
    pub(super) cache: Vec<DocumentLink>,
    pub(super) request: TaskController,
    /// An explicit follow operation has a separate lifetime from cache refreshes.
    follow: TaskController,
    follow_view: Option<ViewId>,
    pub(super) handler: Option<DocumentLinksHandler>,
}

impl DocumentLinks {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.request.cancel();
        self.cancel_navigation();
        self.clear_cache();
    }

    pub(crate) fn bind_handler(&mut self, handler: DocumentLinksHandler) {
        self.cancel_navigation();
        self.handler = Some(handler);
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        self.cancel_navigation();
        changes.update_positions(self.cache.iter_mut().flat_map(|link| {
            std::iter::once((&mut link.start, Assoc::After))
                .chain(std::iter::once((&mut link.end, Assoc::After)))
        }));
    }

    pub(super) fn start_navigation(&mut self, view: ViewId) -> TaskHandle {
        self.follow_view = Some(view);
        self.follow.restart()
    }

    pub(crate) fn cancel_navigation(&mut self) {
        self.follow_view = None;
        self.follow.cancel();
    }

    pub(crate) fn cancel_navigation_for(&mut self, view: ViewId) {
        if self.follow_view == Some(view) {
            self.cancel_navigation();
        }
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

    #[tokio::test]
    async fn requests_outlive_views_and_cache_clears_but_not_the_document() {
        let mut doc = crate::document::test_document("sample text");
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
