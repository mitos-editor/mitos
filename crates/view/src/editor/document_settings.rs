//! Complete document setting changes shared by frontends and background work.

use crate::{DocumentId, Editor};
use anyhow::{anyhow, Context as _, Result};
use language_config::SpellingLanguage;

impl Editor {
    /// Set an explicit language, or plain text with `None`, without changing text or focus.
    pub fn set_document_language(
        &mut self,
        doc_id: DocumentId,
        language: Option<&str>,
    ) -> Result<()> {
        anyhow::ensure!(self.document(doc_id).is_some(), "document is closed");
        let loader = self.syn_loader.load_full();
        let config = match language {
            Some(name) => {
                let language = loader
                    .language_for_name(name)
                    .ok_or_else(|| anyhow!("invalid language id: {name}"))?;
                Some(loader.language(language).config().clone())
            }
            None => None,
        };
        // An explicit language choice starts a new diagnostic session, even if it
        // reuses the client. Configuration refreshes can still re-filter cached results.
        if let Some(uri) = self.document(doc_id).unwrap().uri() {
            self.diagnostics.remove(&uri);
        }
        self.prepare_document_language_change(doc_id);
        let doc = self.documents.get_mut(&doc_id).unwrap();
        doc.set_language(config, &loader);
        doc.detect_indent_and_line_ending();
        self.refresh_document_language_services(doc_id);
        Ok(())
    }

    /// Set dictionary overrides. An empty list disables spelling; `None` restores inheritance.
    pub fn set_document_spelling_languages(
        &mut self,
        doc_id: DocumentId,
        languages: Option<Vec<SpellingLanguage>>,
    ) -> Result<()> {
        let doc = self.document_mut(doc_id).context("document is closed")?;
        doc.spelling_language_override = languages;
        self.refresh_spelling(doc_id);
        Ok(())
    }

    /// Re-detect settings after a path or language configuration change.
    pub fn refresh_doc_language(&mut self, doc_id: DocumentId) {
        if self.document(doc_id).is_none() {
            return;
        }
        let loader = self.syn_loader.load_full();
        self.prepare_document_language_change(doc_id);
        let doc = self.documents.get_mut(&doc_id).unwrap();
        doc.detect_language(&loader);
        doc.detect_editor_config();
        doc.detect_indent_and_line_ending();
        self.refresh_document_language_services(doc_id);
    }

    fn prepare_document_language_change(&mut self, doc_id: DocumentId) {
        if self
            .tree
            .try_get(self.tree.focus())
            .is_some_and(|view| view.doc == doc_id)
        {
            self.handlers.completions.invalidate();
            self.handlers.signature_hints.cancel();
        }
        // Reopen even when the next language uses the same server: didOpen carries languageId.
        self.documents
            .get_mut(&doc_id)
            .unwrap()
            .close_language_server_documents();
    }

    fn refresh_document_language_services(&mut self, doc_id: DocumentId) {
        self.refresh_language_servers(doc_id);
        self.refresh_language_server_diagnostics(doc_id);
        self.refresh_spelling(doc_id);
        event::dispatch(crate::events::DocumentLanguageDidChange {
            editor: self,
            doc: doc_id,
        })
        .report_to(self.status_sender());
    }
}
