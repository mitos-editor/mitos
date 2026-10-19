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

        let breakpoint_paths = self.rename_debugger_sources(old_path, &new_path);
        let documents: Vec<_> = self
            .documents()
            .filter_map(|doc| {
                let suffix = doc.path()?.strip_prefix(old_path).ok()?;
                Some((
                    doc.id(),
                    if suffix.as_os_str().is_empty() {
                        new_path.clone()
                    } else {
                        new_path.join(suffix)
                    },
                    doc.breakpoint_saved_text.clone(),
                ))
            })
            .collect();
        for (id, path, saved) in documents {
            self.set_doc_path(id, &path);
            self.documents.get_mut(&id).unwrap().breakpoint_saved_text = saved;
        }
        self.sync_renamed_debugger_sources(breakpoint_paths);
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
        let doc = self.documents.get_mut(&doc_id).expect("document exists");
        if doc.is_virtual() {
            self.set_error(|| "Adapter source documents cannot be renamed");
            return;
        }
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
        doc.breakpoint_source = Some(self.debugger.breakpoint_source(path));
        doc.breakpoint_saved_text = Some(doc.text().clone());
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
