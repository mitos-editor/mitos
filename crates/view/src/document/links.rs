//! Document links and their request lifetime.

use event::TaskController;
use lsp_client::{lsp, LanguageServerId};

use super::Document;
use crate::handlers::document_links::DocumentLinksHandler;

#[derive(Default)]
pub(crate) struct DocumentLinks {
    pub(crate) cache: Vec<DocumentLink>,
    pub(crate) request: TaskController,
    pub(crate) handler: Option<DocumentLinksHandler>,
}

impl DocumentLinks {
    /// Clear cached navigation targets without interrupting a replacement request.
    pub(crate) fn clear_cache(&mut self) {
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
