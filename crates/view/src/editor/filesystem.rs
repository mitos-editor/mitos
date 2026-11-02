//! Filesystem operations and their editor, watcher, and language-server effects.

mod preflight;
pub use preflight::FileOperation;

use std::{fs, io, path::Path};

use lsp_client::lsp;
use stdx::path::canonicalize;
use workspace::trust::TrustQuery;

use crate::{DocumentId, Editor};

/// How an operation treats an existing destination. Overwrite takes precedence.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExistingTarget {
    Reject,
    Ignore,
    Overwrite,
}

/// Ignored operations leave editor state and protocol observers untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MutationOutcome {
    Applied,
    Ignored,
}

impl ExistingTarget {
    pub(crate) fn from_options(overwrite: bool, ignore: bool) -> Self {
        if overwrite {
            Self::Overwrite
        } else if ignore {
            Self::Ignore
        } else {
            Self::Reject
        }
    }

    fn mutation_outcome(self, result: io::Result<()>) -> io::Result<MutationOutcome> {
        match result {
            Ok(()) => Ok(MutationOutcome::Applied),
            Err(error) if self == Self::Ignore && error.kind() == io::ErrorKind::AlreadyExists => {
                Ok(MutationOutcome::Ignored)
            }
            Err(error) => Err(error),
        }
    }
}

impl Editor {
    /// Apply a resource change and its notifications without requesting preflight edits.
    pub(crate) fn move_path(
        &mut self,
        old_path: &Path,
        new_path: &Path,
        existing: ExistingTarget,
    ) -> io::Result<()> {
        let new_path = canonicalize(new_path);
        // sanity check
        if old_path == new_path {
            return Ok(());
        }
        if !self.prepare_resource_target(&new_path, existing)? {
            return Ok(());
        }
        self.ensure_path_not_writing(old_path)?;
        self.ensure_path_not_writing(&new_path)?;
        let old_watched = self.file_watcher.is_watching(old_path);
        let new_watched = self.file_watcher.is_watching(&new_path);
        let source_exists = match fs::symlink_metadata(old_path) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if source_exists {
            if let Some(parent) = new_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let result = if existing == ExistingTarget::Overwrite {
                fs::rename(old_path, &new_path)
            } else {
                stdx::file::rename_noreplace(old_path, &new_path)
            };
            if existing.mutation_outcome(result)? == MutationOutcome::Ignored {
                return Ok(());
            }
            self.file_operation_revision += 1;
        }

        self.retarget_moved_documents(old_path, &new_path);
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

    /// Retarget documents and debugger sources before notifying language servers.
    fn retarget_moved_documents(&mut self, old_path: &Path, new_path: &Path) {
        let breakpoint_paths = self.rename_debugger_sources(old_path, new_path);
        let documents: Vec<_> = self
            .documents()
            .filter_map(|doc| {
                let suffix = doc.path()?.strip_prefix(old_path).ok()?;
                Some((
                    doc.id(),
                    if suffix.as_os_str().is_empty() {
                        new_path.to_owned()
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
    }

    pub(crate) fn create_path(
        &mut self,
        path: &Path,
        is_dir: bool,
        existing: ExistingTarget,
    ) -> io::Result<()> {
        let path = canonicalize(path);
        if !self.prepare_resource_target(&path, existing)? {
            return Ok(());
        }
        self.ensure_path_not_writing(&path)?;
        if let Some(dir) = path.parent()
            && !dir.is_dir()
        {
            fs::create_dir_all(dir)?;
        }
        let result = if is_dir {
            fs::create_dir(&path)
        } else if existing == ExistingTarget::Overwrite {
            fs::write(&path, [])
        } else {
            fs::File::create_new(&path).map(|_| ())
        };
        if existing.mutation_outcome(result)? == MutationOutcome::Ignored {
            return Ok(());
        }
        self.file_operation_revision += 1;

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

    /// Copy a regular file to a new path, creating missing parent directories.
    /// An existing destination is never overwritten.
    pub(crate) fn copy_path(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let to = canonicalize(to);
        if fs::metadata(from)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!(
                    "Copying directories is not supported: {} is a directory",
                    from.display()
                ),
            ));
        }
        self.prepare_resource_target(&to, ExistingTarget::Reject)?;
        self.ensure_path_not_writing(&to)?;
        if let Some(dir) = to.parent()
            && !dir.is_dir()
        {
            fs::create_dir_all(dir)?;
        }
        let mut source = fs::File::open(from)?;
        // `create_new` keeps a file created after the existence check intact.
        let mut target = fs::File::create_new(&to)?;
        let copied = io::copy(&mut source, &mut target)
            .and_then(|_| target.set_permissions(source.metadata()?.permissions()));
        if let Err(error) = copied {
            drop(target);
            let _ = fs::remove_file(&to);
            return Err(error);
        }
        self.file_operation_revision += 1;

        for ls in self.language_servers.iter_clients() {
            if !ls.is_initialized() {
                continue;
            }
            ls.did_create(&to, false);
        }
        if !self.file_watcher.is_watching(&to) {
            self.language_servers
                .file_event_handler
                .file_changed(to, lsp::FileChangeType::CREATED);
        }
        Ok(())
    }

    /// Increases after each create, move, copy or delete applied by the editor,
    /// so views of the filesystem can refresh even without a file watcher.
    pub fn file_operation_revision(&self) -> u64 {
        self.file_operation_revision
    }

    fn prepare_resource_target(&self, path: &Path, existing: ExistingTarget) -> io::Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(_) => match existing {
                ExistingTarget::Ignore => return Ok(false),
                ExistingTarget::Reject => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("Destination already exists: {}", path.display()),
                    ))
                }
                ExistingTarget::Overwrite => {}
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        // Replacing an open target would create duplicate document identities or
        // leave its buffer disconnected from the file's new contents.
        let resolved = stdx::path::canonicalize_existing(path);
        if self.documents().any(|doc| {
            doc.path().is_some_and(|doc_path| {
                doc_path.starts_with(path)
                    || stdx::path::canonicalize_existing(doc_path).starts_with(&resolved)
            })
        }) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("Destination is open in the editor: {}", path.display()),
            ));
        }
        Ok(true)
    }

    pub(crate) fn delete_path(&mut self, path: &Path, recursive: bool) -> io::Result<()> {
        let path = canonicalize(path);
        let metadata = fs::symlink_metadata(&path)?;
        self.ensure_path_not_writing(&path)?;
        let watched = self.file_watcher.is_watching(&path);
        let is_dir = metadata.is_dir();
        if is_dir {
            if recursive {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_dir(&path)?;
            }
        } else {
            fs::remove_file(&path)?;
        }
        self.file_operation_revision += 1;

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

    pub(crate) fn ensure_path_not_writing(&self, path: &Path) -> io::Result<()> {
        if self.file_is_writing(path) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "A save is pending for {}; wait for it to complete and try again",
                    path.display()
                ),
            ));
        }
        Ok(())
    }

    /// Includes accepted replacement writes and aliases of the same file.
    pub fn file_is_writing(&self, path: &Path) -> bool {
        self.writes.conflicts_with(&stdx::path::canonicalize(path))
            || self
                .writes
                .conflicts_with(&stdx::path::canonicalize_existing(path))
    }

    pub fn set_doc_path(&mut self, doc_id: DocumentId, path: &Path) {
        let doc = self.documents.get_mut(&doc_id).expect("document exists");
        if doc.is_virtual() {
            self.set_error(|| "Adapter source documents cannot be renamed");
            return;
        }
        if doc.is_content_readonly() {
            self.set_error(|| "Review documents cannot be given a file path");
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
        self.refresh_doc_language(doc_id);
        self.refresh_vcs_watches();
    }
}

#[cfg(test)]
mod copy_tests {
    use super::*;
    use crate::editor::tests::editor;

    #[tokio::test]
    async fn copying_a_file_creates_parents_keeps_the_source_and_bumps_the_revision() {
        let (mut editor, directory) = editor("#[|a]#\n");
        let source = directory.path().join("source.txt");
        fs::write(&source, "contents").unwrap();
        let revision = editor.file_operation_revision();
        let target = directory.path().join("new/target.txt");
        editor.copy_path(&source, &target).unwrap();
        assert_eq!(fs::read_to_string(&source).unwrap(), "contents");
        assert_eq!(fs::read_to_string(&target).unwrap(), "contents");
        assert_eq!(editor.file_operation_revision(), revision + 1);
    }

    #[tokio::test]
    async fn copying_never_overwrites_and_rejects_directories() {
        let (mut editor, directory) = editor("#[|a]#\n");
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        fs::write(&source, "new").unwrap();
        fs::write(&target, "old").unwrap();
        let revision = editor.file_operation_revision();
        let error = editor.copy_path(&source, &target).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");

        let error = editor
            .copy_path(directory.path(), &directory.path().join("copy"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
        assert!(!directory.path().join("copy").exists());
        assert_eq!(editor.file_operation_revision(), revision);
    }

    #[tokio::test]
    async fn create_move_and_delete_bump_the_revision_once_each() {
        let (mut editor, directory) = editor("#[|a]#\n");
        let file = directory.path().join("file.txt");
        let moved = directory.path().join("moved.txt");
        let start = editor.file_operation_revision();
        editor
            .create_path(&file, false, ExistingTarget::Reject)
            .unwrap();
        editor
            .move_path(&canonicalize(&file), &moved, ExistingTarget::Reject)
            .unwrap();
        editor.delete_path(&moved, false).unwrap();
        assert!(!moved.exists());
        assert_eq!(editor.file_operation_revision(), start + 3);
    }
}

#[cfg(test)]
mod parent_directory_tests {
    use super::*;
    use crate::editor::{tests::editor, Action};

    #[tokio::test]
    async fn moving_a_file_creates_missing_parent_directories_and_retargets_its_document() {
        let (mut editor, directory) = editor("#[|a]#\n");
        let source = directory.path().join("source.txt");
        fs::write(&source, "contents").unwrap();
        // Match the path normalization performed by file-operation preflight.
        let source = canonicalize(source);
        let id = editor.open(&source, Action::Replace).unwrap();
        let target = directory.path().join("new/nested/target.txt");
        editor
            .move_path(&source, &target, ExistingTarget::Reject)
            .unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&target).unwrap(), "contents");
        assert_eq!(
            editor.document(id).unwrap().path(),
            Some(stdx::path::canonicalize(&target).as_path())
        );
    }
}
