//! Document-owned code lens cache and request lifetimes.

use editor_core::{Assoc, ChangeSet};
use event::TaskController;
use lsp_client::{lsp, LanguageServerId};
use std::collections::HashMap;

use super::DocumentCodeLensesHandler;
use crate::Document;

#[derive(Default)]
pub(crate) struct DocumentCodeLenses {
    pub(super) cache: Vec<CodeLens>,
    pub(super) version: Option<i32>,
    pub(super) uri: Option<editor_core::Uri>,
    pub(super) request: TaskController,
    pub(super) resolve: TaskController,
    /// One automatic attempt per lens and cache generation, including failures.
    /// Dropping these controllers cancels queued requests and publications.
    pub(super) titles: HashMap<usize, TaskController>,
    pub(super) handler: Option<DocumentCodeLensesHandler>,
}

impl DocumentCodeLenses {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            version: self.version,
            uri: self.uri.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn bind_handler(&mut self, handler: DocumentCodeLensesHandler) {
        self.handler = Some(handler);
    }

    /// Keep cached anchors consistent before observers run.
    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        changes.update_positions(
            self.cache
                .iter_mut()
                .map(|lens| (&mut lens.char_idx, Assoc::After)),
        );
        self.cancel_requests();
    }

    /// Cancel both requests and queued publications, retaining mapped positions.
    pub(super) fn cancel_requests(&mut self) {
        self.request.cancel();
        self.resolve.cancel();
        self.titles.clear();
    }

    pub(crate) fn clear(&mut self) {
        self.cancel_requests();
        self.cache.clear();
        self.version = None;
        self.uri = None;
    }
}

#[derive(Debug, Clone)]
pub struct CodeLens {
    /// Character offset in the current document, mapped through edits.
    pub char_idx: usize,
    /// Original protocol value retained for lazy resolution, including opaque data.
    pub lens: lsp::CodeLens,
    pub language_server_id: LanguageServerId,
}

impl Document {
    /// Cached code lenses ordered by document position and originating server.
    pub fn code_lenses(&self) -> &[CodeLens] {
        &self.code_lenses.cache
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropping_a_document_cancels_fetch_resolution_and_title_requests() {
        let mut doc = crate::document::test_document("sample text");
        let mut title_controller = TaskController::new();
        let title = title_controller.restart();
        doc.code_lenses.titles.insert(0, title_controller);
        let requests = [
            doc.code_lenses.request.restart(),
            doc.code_lenses.resolve.restart(),
            title,
        ];
        drop(doc);
        assert!(requests.iter().all(event::TaskHandle::is_canceled));
    }
}
