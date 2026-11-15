//! Shared identity and opening for immutable, provider-owned source buffers.
//! Providers own loading and invalidation; reviews retain their comparison model.

use std::collections::HashMap;
#[cfg(feature = "git")]
use std::path::PathBuf;

use anyhow::{Context, Result};
use editor_core::Rope;
use event::register_hook;
use lsp_client::LanguageServerId;

use crate::{editor::Action, events::DocumentDidClose, Document, DocumentId, Editor};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SourceDocumentKey {
    LanguageServer(LanguageServerId, String),
    #[cfg(feature = "git")]
    Revision {
        repository: PathBuf,
        revision: vcs::operations::history::RevisionId,
        path: PathBuf,
    },
}

#[derive(Default)]
pub(crate) struct SourceDocuments {
    documents: HashMap<SourceDocumentKey, DocumentId>,
}

impl SourceDocuments {
    pub(super) fn remove_server(&mut self, server: LanguageServerId) {
        self.documents.retain(|key, _| {
            !matches!(key, SourceDocumentKey::LanguageServer(owner, _) if *owner == server)
        });
    }

    pub(super) fn remove_language_servers(&mut self) {
        self.documents
            .retain(|key, _| !matches!(key, SourceDocumentKey::LanguageServer(..)));
    }
}

impl Editor {
    pub(crate) fn source_document(&self, key: &SourceDocumentKey) -> Option<DocumentId> {
        let id = *self.source_documents.documents.get(key)?;
        self.document(id).map(|_| id)
    }

    /// Reuse a provider identity, or install a fresh immutable document.
    /// Anonymous snapshots always open independently. The boolean identifies a
    /// newly installed document whose provider may need to start loading.
    pub(crate) fn open_source_document(
        &mut self,
        key: Option<SourceDocumentKey>,
        action: Action,
        create: impl FnOnce(&Editor) -> Document,
    ) -> (DocumentId, bool) {
        if let Some(id) = key.as_ref().and_then(|key| self.source_document(key)) {
            self.switch(id, action);
            return (id, false);
        }
        let doc = create(self);
        assert!(doc.is_content_readonly() && doc.path().is_none());
        let id = self.new_file_from_document(action, doc);
        if let Some(key) = key {
            self.source_documents.documents.insert(key, id);
        }
        self.events()
            .clone()
            .dispatch(crate::events::DocumentDidOpen {
                editor: self,
                doc: id,
            })
            .report_to(self.status_sender());
        (id, true)
    }

    /// Publish provider content without creating undo history or modified state.
    pub(crate) fn replace_source_document_content(
        &mut self,
        id: DocumentId,
        text: &Rope,
        initialize: bool,
    ) -> Result<()> {
        let doc = self
            .document_mut(id)
            .context("Source document was closed")?;
        assert!(doc.is_content_readonly());
        if doc.text() != text {
            let changes = text_diff::compare_ropes(doc.text(), text);
            doc.replace_readonly_content(changes.changes(), initialize)?;
        }
        Ok(())
    }

    /// Failed loads can be retried, and closed buffers cannot retain cache entries.
    pub(crate) fn forget_source_document(&mut self, document: DocumentId) {
        self.source_documents.documents.retain(|key, id| {
            if *id != document {
                return true;
            }
            match key {
                SourceDocumentKey::LanguageServer(server, uri) => {
                    self.virtual_documents.remove(*server, uri)
                }
                #[cfg(feature = "git")]
                SourceDocumentKey::Revision { .. } => {}
            }
            false
        });
    }
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut DocumentDidClose<'_>| {
        event.editor.forget_source_document(event.doc.id());
        Ok(())
    });
}
