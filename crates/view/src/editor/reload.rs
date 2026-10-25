//! Shared reload effects. Explicit reloads supply fallback file notifications;
//! automatic reloads reuse the notification already sent by their event source.

use crate::{DocumentId, Editor};
use workspace::trust::TrustQuery;

impl Editor {
    /// Reload explicitly, notifying language servers when native watching does not cover the path.
    /// Failures send no synthetic file notification.
    pub fn reload_document(&mut self, id: DocumentId) -> anyhow::Result<()> {
        self.reload_document_after_file_change(id)?;
        if let Some(path) = self.document(id).and_then(|doc| doc.path())
            && !self.file_watcher.is_watching(path)
        {
            self.language_servers
                .file_event_handler
                .file_changed(path.to_owned(), lsp_client::lsp::FileChangeType::CHANGED);
        }
        Ok(())
    }

    /// Reload after a native watcher or poller has already delivered the file notification.
    pub(crate) fn reload_document_after_file_change(
        &mut self,
        id: DocumentId,
    ) -> anyhow::Result<()> {
        self.sync_document_views(id);
        let scrolloff = self.config().scroll.padding;
        let doc = self
            .documents
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Document is closed"))?;
        let trusted = self
            .workspace_trust
            .query(doc.workspace_root(), TrustQuery::Git)
            .is_trusted();
        doc.reload(trusted)?;
        doc.breakpoint_saved_text = Some(doc.text().clone());
        let debugger_path = doc.path().map(|path| path.to_owned());
        for (view, _) in self.tree.views_mut() {
            if view.doc == id {
                view.sync_changes(doc);
                view.ensure_cursor_in_view(doc, scrolloff);
            }
        }
        if let Some(path) = debugger_path {
            self.debugger_source_saved(&path);
        }
        Ok(())
    }
}
