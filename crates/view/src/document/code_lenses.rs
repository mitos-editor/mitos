//! Document-owned code lens cache and request lifetimes.

use editor_core::{Assoc, ChangeSet, Rope};
use event::TaskController;
use lsp_client::{lsp, LanguageServerId};
use std::collections::HashMap;

use super::Document;
use crate::handlers::code_lenses::DocumentCodeLensesHandler;

#[derive(Default)]
pub(crate) struct DocumentCodeLenses {
    pub(crate) cache: Vec<CodeLens>,
    pub(crate) version: Option<i32>,
    pub(crate) uri: Option<editor_core::Uri>,
    /// Server-visible text while temporary transactions preview a different buffer.
    pub(crate) preview_base: Option<editor_core::Rope>,
    pub(crate) request: TaskController,
    pub(crate) resolve: TaskController,
    /// One automatic attempt per lens and cache generation, including failures.
    /// Dropping these controllers cancels queued requests and publications.
    pub(crate) titles: HashMap<usize, TaskController>,
    pub(crate) handler: Option<DocumentCodeLensesHandler>,
}

impl DocumentCodeLenses {
    /// Keep cached anchors and preview ownership consistent before observers run.
    pub(super) fn apply_changes(
        &mut self,
        changes: &ChangeSet,
        old_text: &Rope,
        text: &Rope,
        temporary: bool,
    ) {
        changes.update_positions(
            self.cache
                .iter_mut()
                .map(|lens| (&mut lens.char_idx, Assoc::After)),
        );
        self.cancel_requests();
        if temporary {
            self.preview_base.get_or_insert_with(|| old_text.clone());
            if self.preview_base.as_ref().is_some_and(|base| base == text) {
                self.preview_base = None;
            }
        } else {
            self.preview_base = None;
        }
    }

    /// Cancel both requests and queued publications, retaining mapped positions.
    pub(crate) fn cancel_requests(&mut self) {
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
