//! Filesystem operations and their editor, watcher, and language-server effects.

mod preflight;
pub use preflight::FileOperation;

use std::{fs, io, path::Path};

use lsp_client::lsp;
use stdx::path::canonicalize;
use workspace::trust::TrustQuery;

use crate::{DocumentId, Editor};

impl Editor {
    /// Apply a resource change and its notifications without requesting preflight edits.
    pub(crate) fn move_path(&mut self, old_path: &Path, new_path: &Path) -> io::Result<()> {
        let new_path = canonicalize(new_path);
        // sanity check
        if old_path == new_path {
            return Ok(());
        }
        let old_watched = self.file_watcher.is_watching(old_path);
        let new_watched = self.file_watcher.is_watching(&new_path);
        if old_path.exists() {
            fs::rename(old_path, &new_path)?;
        }

        if let Some(doc) = self.document_by_path(old_path) {
            self.set_doc_path(doc.id(), &new_path);
        }
        let is_dir = new_path.is_dir();
        for ls in self.language_servers.iter_clients() {
            // A new language server might have been started in `set_doc_path` and won't
            // be initialized yet. Skip the `did_rename` notification for this server.
            if !ls.is_initialized() {
                continue;
            }
            ls.did_rename(old_path, &new_path, is_dir);
        }

        if !old_watched {
            self.language_servers
                .file_event_handler
                .file_changed(old_path.to_owned(), lsp::FileChangeType::DELETED);
        }
        if !new_watched {
            self.language_servers
                .file_event_handler
                .file_changed(new_path, lsp::FileChangeType::CREATED);
        }
        Ok(())
    }

    pub(crate) fn create_path(&mut self, path: &Path, is_dir: bool) -> io::Result<()> {
        let path = canonicalize(path);
        if let Some(dir) = path.parent()
            && !dir.is_dir()
        {
            fs::create_dir_all(dir)?;
        }
        if is_dir {
            fs::create_dir(&path)?;
        } else {
            fs::write(&path, [])?;
        }

        for ls in self.language_servers.iter_clients() {
            if !ls.is_initialized() {
                continue;
            }
            ls.did_create(&path, is_dir);
        }
        if !self.file_watcher.is_watching(&path) {
            self.language_servers
                .file_event_handler
                .file_changed(path, lsp::FileChangeType::CREATED);
        }
        Ok(())
    }

    pub(crate) fn delete_path(&mut self, path: &Path, recursive: bool) -> io::Result<()> {
        let path = canonicalize(path);
        let watched = self.file_watcher.is_watching(&path);
        let is_dir = path.is_dir();
        if is_dir {
            if recursive {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_dir(&path)?;
            }
        } else {
            fs::remove_file(&path)?;
        }

        for ls in self.language_servers.iter_clients() {
            if !ls.is_initialized() {
                continue;
            }
            ls.did_delete(&path, is_dir);
        }
        if !watched {
            self.language_servers
                .file_event_handler
                .file_changed(path, lsp::FileChangeType::DELETED);
        }
        Ok(())
    }

    pub fn set_doc_path(&mut self, doc_id: DocumentId, path: &Path) {
        let doc = doc_mut!(self, &doc_id);
        let old_path = doc.path();

        if let Some(old_path) = old_path {
            // sanity check, should not occur but some callers (like an LSP) may
            // create bogus calls
            if old_path == path {
                return;
            }
        }
        doc.close_language_server_documents();
        doc.language_servers.clear();
        doc.set_path(Some(path));
        let trust_full = self
            .workspace_trust
            .query(doc.workspace_root(), TrustQuery::Git)
            .is_trusted();
        doc.refresh_vcs(&self.diff_providers, trust_full);
        doc.detect_editor_config();
        self.refresh_doc_language(doc_id);
        self.refresh_vcs_watches();
    }
}
