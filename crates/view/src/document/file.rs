//! File identity, encoding, saved state, and document persistence.
//!
//! File fields stay private here so path changes invalidate every derived cache.

use super::{
    file_io::{read_file, DecodedFile},
    Document, EditorConfig, SCRATCH_BUFFER_NAME,
};
use crate::{DocumentId, Editor};
use anyhow::{anyhow, bail, Error};
use arc_swap::access::DynAccess;
use editor_config::Config;
use editor_core::{encoding, encoding::Encoding, Rope};
use futures_util::future::BoxFuture;
use language::syntax;
use std::{
    borrow::Cow,
    future::Future,
    io,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::SystemTime,
};
use stdx::{
    faccess::{copy_metadata, readonly},
    Url,
};
use text_io::to_writer;
use vcs::DiffProviderRegistry;

/// The text, revision, and destination confirmed by a completed write.
#[derive(Debug, Clone)]
pub struct DocumentSavedEvent {
    pub revision: usize,
    pub save_time: SystemTime,
    pub doc_id: DocumentId,
    pub path: PathBuf,
    pub text: Rope,
    pub(crate) reservation: Option<std::sync::Arc<crate::save::WriteReservation>>,
}

pub type DocumentSavedEventResult = Result<DocumentSavedEvent, anyhow::Error>;
pub type DocumentSavedEventFuture = BoxFuture<'static, DocumentSavedEventResult>;

#[derive(Debug, thiserror::Error)]
pub enum DocumentOpenError {
    #[error("path must be a regular file, symlink, or directory")]
    IrregularFile,
    #[error(transparent)]
    IoError(#[from] io::Error),
}

#[derive(Debug, Clone)]
pub(super) struct FileState {
    path: Option<PathBuf>,
    relative_path: OnceLock<Option<PathBuf>>,
    display_path: OnceLock<Option<PathBuf>>,
    workspace_root: OnceLock<PathBuf>,
    encoding: &'static Encoding,
    has_bom: bool,
    binary: bool,
    readonly: bool,
    readonly_name: Option<String>,
    // The last read or successful save, used to detect external changes.
    last_saved_time: SystemTime,
    last_saved_revision: usize,
    // External change already surfaced while the document remains modified.
    auto_reload_seen_mtime: Option<SystemTime>,
}

impl FileState {
    pub(super) fn new(encoding: &'static Encoding, has_bom: bool) -> Self {
        Self {
            path: None,
            relative_path: OnceLock::new(),
            display_path: OnceLock::new(),
            workspace_root: OnceLock::new(),
            encoding,
            has_bom,
            binary: false,
            readonly: false,
            readonly_name: None,
            last_saved_time: SystemTime::now(),
            last_saved_revision: 0,
            auto_reload_seen_mtime: None,
        }
    }

    fn set_path(&mut self, path: Option<&Path>) {
        self.path = path.map(stdx::path::canonicalize);
        self.relative_path.take();
        self.display_path.take();
        self.workspace_root.take();
    }
}

impl Editor {
    pub(crate) fn clear_doc_relative_paths(&mut self) {
        for doc in self.documents_mut() {
            doc.file.relative_path.take();
        }
    }
}

impl Document {
    /// Name an immutable, pathless document supplied by an external service.
    pub(crate) fn with_readonly_name(mut self, name: String) -> Self {
        self.file.readonly_name = Some(name);
        self.detect_readonly();
        self
    }

    pub(crate) fn is_virtual(&self) -> bool {
        self.file.readonly_name.is_some()
    }

    /// Load an unregistered preview and prepare its cancellable syntax request.
    pub fn open_for_preview(
        path: &Path,
        config: Arc<dyn DynAccess<Config>>,
        syntax_loader: Arc<syntax::Loader>,
    ) -> Result<Self, DocumentOpenError> {
        let mut doc = Self::open(path, None, false, config, syntax_loader)?;
        let loader = doc.syntax_loader.clone();
        doc.detect_language(&loader);
        Ok(doc)
    }

    /// Whether the file is read-only, including binary content.
    pub fn is_readonly(&self) -> bool {
        self.file.readonly || self.is_content_readonly()
    }

    /// External modification already surfaced by automatic reload.
    pub fn auto_reload_seen_mtime(&self) -> Option<SystemTime> {
        self.file.auto_reload_seen_mtime
    }

    pub(crate) fn set_auto_reload_seen_mtime(&mut self, mtime: Option<SystemTime>) {
        self.file.auto_reload_seen_mtime = mtime;
    }

    /// Create a new document from `path`. Encoding is auto-detected, but it can be manually
    /// overwritten with the `encoding` parameter.
    /// The editor requests syntax initialization after assigning the document an id.
    pub fn open(
        path: &Path,
        mut encoding: Option<&'static Encoding>,
        detect_language: bool,
        config: Arc<dyn DynAccess<Config>>,
        syntax_loader: Arc<syntax::Loader>,
    ) -> Result<Self, DocumentOpenError> {
        // If the path is not a regular file (e.g.: /dev/random) it should not be opened.
        if path.metadata().is_ok_and(|metadata| !metadata.is_file()) {
            return Err(DocumentOpenError::IrregularFile);
        }

        let editor_config = if config.load().files.editor_config {
            EditorConfig::find(path)
        } else {
            EditorConfig::default()
        };
        encoding = encoding.or(editor_config.encoding);

        // Open the file if it exists, otherwise assume it is a new file (and thus empty).
        let DecodedFile {
            text,
            encoding,
            has_bom,
            binary,
        } = if path.exists() {
            let mut file = std::fs::File::open(path)?;
            read_file(&mut file, encoding)?
        } else {
            let line_ending = editor_config
                .line_ending
                .unwrap_or_else(|| config.load().files.default_line_ending.into());
            let encoding = encoding.unwrap_or(encoding::UTF_8);
            DecodedFile {
                text: Rope::from(line_ending.as_str()),
                encoding,
                has_bom: false,
                binary: false,
            }
        };

        let mut doc = Self::from(text, Some((encoding, has_bom)), config, syntax_loader);

        doc.file.binary = binary;
        // set the path and try detecting the language
        doc.set_path(Some(path));
        if detect_language {
            // The editor requests syntax after assigning a stable document id.
            doc.language = doc.detect_language_config(&doc.syntax_loader);
        }

        doc.editor_config = editor_config;
        doc.detect_indent_and_line_ending();

        Ok(doc)
    }

    /// Load a text file as an unregistered document without a filesystem identity.
    pub(crate) fn open_scratch(
        path: &Path,
        config: Arc<dyn DynAccess<Config>>,
        syntax_loader: Arc<syntax::Loader>,
    ) -> Result<Self, Error> {
        if !path.metadata()?.is_file() {
            return Err(DocumentOpenError::IrregularFile.into());
        }
        let mut doc = Self::open(path, None, true, config, syntax_loader)?;
        anyhow::ensure!(
            !doc.is_binary(),
            "Cannot open a binary file as scratch text"
        );
        doc.set_path(None);
        doc.refresh_settings();
        Ok(doc)
    }

    pub fn save<P: Into<PathBuf>>(
        &mut self,
        path: Option<P>,
        force: bool,
    ) -> Result<
        impl Future<Output = Result<DocumentSavedEvent, anyhow::Error>> + 'static + Send + use<P>,
        anyhow::Error,
    > {
        let path = path.map(|path| path.into());
        self.save_impl(path, force)
    }

    /// The `Document`'s text is encoded according to its encoding and written to the file located
    /// at its `path()`.
    fn save_impl(
        &mut self,
        path: Option<PathBuf>,
        force: bool,
    ) -> Result<
        impl Future<Output = Result<DocumentSavedEvent, anyhow::Error>> + 'static + Send + use<>,
        anyhow::Error,
    > {
        if self.is_virtual() {
            bail!("Adapter source documents cannot be saved; copy the text to a new buffer first");
        }
        anyhow::ensure!(
            !self.is_content_readonly(),
            "Review documents cannot be written, including with force or a new filename"
        );
        if self.file.binary {
            bail!("Binary files are read-only and cannot be saved as text");
        }
        log::debug!(
            "submitting save of doc '{:?}'",
            self.path().map(|path| path.to_string_lossy())
        );

        // we clone and move text + path into the future so that we asynchronously save the current
        // state without blocking any further edits.
        let snapshot = self.snapshot();

        let path = match path {
            Some(path) => stdx::path::canonicalize(path),
            None => self
                .path()
                .ok_or_else(|| anyhow!("Can't save with no path set!"))?
                .to_owned(),
        };

        let identifier = self.path().map(|_| self.identifier());
        let language_servers: Vec<_> = self
            .language_servers
            .values()
            .filter(|server| self.opened_language_servers.contains(&server.id()))
            .cloned()
            .collect();

        let doc_id = self.id();
        let atomic_save = self.config.load().files.save.atomic;

        let encoding_with_bom_info = (self.file.encoding, self.file.has_bom);
        let last_saved_time = self.file.last_saved_time;

        // We encode the file according to the `Document`'s encoding.
        let future = async move {
            use tokio::fs;
            let text = snapshot.text();
            if let Some(parent) = path.parent() {
                // TODO: display a prompt asking the user if the directories should be created
                if !parent.exists() {
                    if force {
                        std::fs::DirBuilder::new().recursive(true).create(parent)?;
                    } else {
                        bail!("can't save file, parent directory does not exist (use :w! to create it)");
                    }
                }
            }

            // Protect against overwriting changes made externally
            if !force
                && let Ok(metadata) = fs::metadata(&path).await
                && let Ok(mtime) = metadata.modified()
                && last_saved_time < mtime
            {
                bail!("file modified by an external process, use :w! to overwrite");
            }
            let write_path = tokio::fs::read_link(&path)
                .await
                .ok()
                .and_then(|p| {
                    if p.is_relative() {
                        path.parent().map(|parent| parent.join(p))
                    } else {
                        Some(p)
                    }
                })
                .unwrap_or_else(|| path.clone());

            if readonly(&write_path) {
                bail!(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Path is read only"
                ));
            }

            // Assume it is a hardlink to prevent data loss if the metadata cant be read (e.g. on certain Windows configurations)
            let is_hardlink = stdx::faccess::hardlink_count(&write_path).unwrap_or(2) > 1;
            let is_symlink = match tokio::fs::symlink_metadata(&write_path).await {
                Ok(meta) => meta.is_symlink(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
                Err(err) => return Err(err.into()),
            };
            let must_copy = is_hardlink || is_symlink;
            let backup = if path.exists() && atomic_save {
                let path_ = write_path.clone();
                // hacks: we use tempfile to handle the complex task of creating
                // non clobbered temporary path for us we don't want
                // the whole automatically delete path on drop thing
                // since the path doesn't exist yet, we just want
                // the path
                tokio::task::spawn_blocking(move || -> Option<PathBuf> {
                    let mut builder = tempfile::Builder::new();
                    builder.prefix(path_.file_name()?).suffix(".bck");

                    let backup_path = if must_copy {
                        builder
                            .make_in(path_.parent()?, |backup| std::fs::copy(&path_, backup))
                            .ok()?
                            .into_temp_path()
                    } else {
                        builder
                            .make_in(path_.parent()?, |backup| std::fs::rename(&path_, backup))
                            .ok()?
                            .into_temp_path()
                    };

                    backup_path.keep().ok()
                })
                .await
                .ok()
                .flatten()
            } else {
                None
            };

            let write_result: anyhow::Result<_> = async {
                let mut dst = tokio::fs::File::create(&write_path).await?;
                to_writer(&mut dst, encoding_with_bom_info, text).await?;
                // Ignore ENOTSUP/EOPNOTSUPP (Operation not supported) errors from sync_all()
                // This is known to occur on SMB filesystems on macOS where fsync is not supported
                match dst.sync_all().await {
                    Ok(_) => (),
                    Err(err) if err.kind() == io::ErrorKind::Unsupported => (),
                    // Some extra OS errors are thrown on macOS for example if fsync is not
                    // available for this filesystem. NOTE: on macOS, ENOTSUP and EOPNOTSUPP are
                    // not the same code, so we need to suppress the unreachable_patterns lint on
                    // Unix generally.
                    #[allow(unreachable_patterns)]
                    #[cfg(unix)]
                    Err(err)
                        if matches!(err.raw_os_error(), Some(libc::ENOTSUP | libc::EOPNOTSUPP)) => {
                    }
                    Err(err) => return Err(err.into()),
                }
                Ok(())
            }
            .await;

            let save_time = match fs::metadata(&write_path).await {
                Ok(metadata) => metadata.modified().map_or(SystemTime::now(), |mtime| mtime),
                Err(_) => SystemTime::now(),
            };

            if let Some(backup) = backup {
                if must_copy {
                    let mut delete = true;
                    if write_result.is_err() {
                        // Restore backup
                        let _ = tokio::fs::copy(&backup, &write_path).await.map_err(|e| {
                            delete = false;
                            log::error!("Failed to restore backup on write failure: {e}")
                        });
                    }

                    if delete {
                        // Delete backup
                        let _ = tokio::fs::remove_file(backup)
                            .await
                            .map_err(|e| log::error!("Failed to remove backup file on write: {e}"));
                    }
                } else if write_result.is_err() {
                    // restore backup
                    let _ = tokio::fs::rename(&backup, &write_path)
                        .await
                        .map_err(|e| log::error!("Failed to restore backup on write failure: {e}"));
                } else {
                    // copy metadata and delete backup
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = copy_metadata(&backup, &write_path)
                            .map_err(|e| log::error!("Failed to copy metadata on write: {e}"));
                        let _ = std::fs::remove_file(backup)
                            .map_err(|e| log::error!("Failed to remove backup file on write: {e}"));
                    })
                    .await;
                }
            }

            write_result?;

            let event = DocumentSavedEvent {
                revision: snapshot.revision(),
                save_time,
                doc_id,
                path,
                text: text.clone(),
                reservation: None,
            };

            for language_server in language_servers {
                if !language_server.is_initialized() {
                    continue;
                }
                if let Some(id) = identifier.clone() {
                    language_server.text_document_did_save(id, text);
                }
            }

            Ok(event)
        };

        Ok(future)
    }

    /// Binary files have no editable text; their original bytes remain on disk.
    pub fn is_binary(&self) -> bool {
        self.file.binary
    }

    /// Filesystem modification time recorded by the last read or successful save.
    pub fn last_saved_time(&self) -> SystemTime {
        self.file.last_saved_time
    }

    pub fn pickup_last_saved_time(&mut self) {
        self.file.auto_reload_seen_mtime = None;
        self.file.last_saved_time = match self.path() {
            Some(path) => match path.metadata() {
                Ok(metadata) => match metadata.modified() {
                    Ok(mtime) => mtime,
                    Err(err) => {
                        log::debug!("Could not fetch file system's mtime, falling back to current system time: {}", err);
                        SystemTime::now()
                    }
                },
                Err(err) => {
                    log::debug!("Could not fetch file system's mtime, falling back to current system time: {}", err);
                    SystemTime::now()
                }
            },
            None => SystemTime::now(),
        };
    }

    // Detect if the file is readonly and change the readonly field if necessary (unix only)
    pub fn detect_readonly(&mut self) {
        // Allows setting the flag for files the user cannot modify, like root files
        self.file.readonly = self.is_virtual()
            || self.file.binary
            || self.file.path.as_ref().is_some_and(|p| readonly(p));
    }

    /// Reload the document from its path.
    pub fn reload(
        &mut self,
        provider_registry: &DiffProviderRegistry,
        trust_full: bool,
    ) -> Result<(), Error> {
        anyhow::ensure!(
            !self.is_content_readonly(),
            "Use :diff-refresh to refresh a review document"
        );
        let encoding = self.file.encoding;
        let path = match self.path() {
            None => return Ok(()),
            Some(path) => match path.exists() {
                true => path.to_owned(),
                false => bail!("can't find file to reload from {:?}", self.display_name()),
            },
        };

        // Once we have a valid path we check if its readonly status has changed
        self.detect_readonly();

        let mut file = std::fs::File::open(&path)?;
        let DecodedFile {
            text: rope, binary, ..
        } = read_file(&mut file, Some(encoding))?;
        self.accepted_completions.cancel();
        self.shell_filters.clear();
        // Reload may change the content type. Allow this replacement, then restore
        // the binary write guard before returning to the editor.
        let was_binary = std::mem::replace(&mut self.file.binary, false);

        // Calculate the difference between the buffer and source text, and apply it.
        // This is not considered a modification of the contents of the file regardless
        // of the encoding.
        let transaction = text_diff::compare_ropes(self.text(), &rope);
        self.apply(transaction.changes())?;
        self.commit_changes();
        self.file.binary = binary;
        self.detect_readonly();
        if binary {
            self.hover.clear();
            self.navigation.clear();
            self.document_links.cancel_navigation();
            self.reset_all_inlay_hints();
            self.language = None;
            self.syntax = None;
            self.pending_syntax = None;
            self.close_language_server_documents();
            self.language_servers.clear();
        } else if was_binary {
            let loader = self.syntax_loader.clone();
            self.detect_language(&loader);
        }
        self.reset_modified();
        self.pickup_last_saved_time();
        self.refresh_settings();

        self.refresh_vcs(provider_registry, trust_full);

        Ok(())
    }

    /// Sets the [`Document`]'s encoding with the encoding correspondent to `label`.
    pub fn set_encoding(&mut self, label: &str) -> Result<(), Error> {
        let encoding =
            Encoding::for_label(label.as_bytes()).ok_or_else(|| anyhow!("unknown encoding"))?;

        self.file.encoding = encoding;

        Ok(())
    }

    /// Returns the [`Document`]'s current encoding.
    pub fn encoding(&self) -> &'static Encoding {
        self.file.encoding
    }

    pub fn has_bom(&self) -> bool {
        self.file.has_bom
    }

    /// sets the document path without sending events to various
    /// observers (like LSP), in most cases `Editor::set_doc_path`
    /// should be used instead
    pub(crate) fn set_path(&mut self, path: Option<&Path>) {
        if self.is_content_readonly() {
            return;
        }
        self.invalidate_blame();
        self.hover.clear();
        self.navigation.clear();
        self.document_links.cancel_navigation();
        self.reset_all_inlay_hints();
        self.shell_filters.clear();
        self.file.set_path(path);

        self.detect_readonly();
        self.pickup_last_saved_time();
    }

    /// If there are unsaved modifications.
    pub fn is_modified(&self) -> bool {
        if self.is_content_readonly() {
            return false;
        }
        let current_revision = self.buffer.current_revision();
        log::debug!(
            "id {} modified - last saved: {}, current: {}",
            self.id,
            self.file.last_saved_revision,
            current_revision
        );
        current_revision != self.file.last_saved_revision || !self.buffer.changes().is_empty()
    }

    /// Save modifications to history, and so [`Self::is_modified`] will return false.
    pub fn reset_modified(&mut self) {
        let current_revision = self.buffer.current_revision();
        self.file.last_saved_revision = current_revision;
    }

    /// Set the document's latest saved revision to the given one.
    pub fn set_last_saved_revision(&mut self, rev: usize, save_time: SystemTime) {
        log::debug!(
            "doc {} revision updated {} -> {}",
            self.id,
            self.file.last_saved_revision,
            rev
        );
        self.file.last_saved_revision = rev;
        self.file.last_saved_time = save_time;
        self.file.auto_reload_seen_mtime = None;
    }

    /// Get the document's latest saved revision.
    pub fn get_last_saved_revision(&self) -> usize {
        self.file.last_saved_revision
    }

    #[inline]
    /// File path on disk.
    pub fn path(&self) -> Option<&Path> {
        self.file.path.as_deref()
    }

    /// File path as a URL.
    pub fn url(&self) -> Option<Url> {
        Url::from_file_path(self.path()?).ok()
    }

    pub fn uri(&self) -> Option<editor_core::Uri> {
        Some(self.path()?.into())
    }

    pub fn relative_path(&self) -> Option<&Path> {
        self.file
            .relative_path
            .get_or_init(|| {
                self.file
                    .path
                    .as_ref()
                    .map(|path| stdx::path::get_relative_path(path).to_path_buf())
            })
            .as_deref()
    }

    /// The workspace root for this document — the nearest ancestor that contains a `.git`, `.svn`,
    /// `.jj`, or `.mitos`. Falls back to the current working directory's workspace when the
    /// document has no path (scratch buffers). Lazily memoised on first call.
    pub fn workspace_root(&self) -> &Path {
        #[cfg(feature = "git")]
        if let Some(commit) = self.commit_message() {
            return commit.workspace();
        }
        if let Some(review) = self.review() {
            return review.workspace();
        }
        if let Some(source) = &self.historical {
            return &source.workspace;
        }
        self.file
            .workspace_root
            .get_or_init(|| match self.file.path.as_deref() {
                Some(p) => p
                    .parent()
                    .map(|dir| workspace::find_workspace_in(dir).0)
                    .unwrap_or_else(|| workspace::find_workspace().0),
                None => workspace::find_workspace().0,
            })
    }

    /// A stable path for user-facing output, relative to this document's workspace when possible.
    pub fn display_path(&self) -> Option<&Path> {
        self.file
            .display_path
            .get_or_init(|| {
                self.file.path.as_deref().map(|path| {
                    stdx::path::get_relative_path_from(path, self.workspace_root()).into_owned()
                })
            })
            .as_deref()
    }

    pub fn display_name(&self) -> Cow<'_, str> {
        #[cfg(feature = "git")]
        if let Some(commit) = self.commit_message() {
            return commit.name().into();
        }
        if let Some(name) = &self.file.readonly_name {
            return name.as_str().into();
        }
        if let Some(review) = self.review() {
            let path = review
                .path()
                .strip_prefix(review.workspace())
                .unwrap_or(review.path());
            return self.display_metadata().format_name(path).into();
        }
        if let Some(source) = &self.historical {
            let path = source
                .path
                .strip_prefix(&source.workspace)
                .unwrap_or(&source.path);
            return self.display_metadata().format_name(path).into();
        }
        self.display_path()
            .map_or_else(|| SCRATCH_BUFFER_NAME.into(), |path| path.to_string_lossy())
    }
}
