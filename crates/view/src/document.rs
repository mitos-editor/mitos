pub use crate::features::formatting::FormatterError;
pub(crate) mod history;
mod lifecycle;
mod mutation;

pub use crate::features::blame::LineBlameError;
use crate::features::{
    blame::DocumentBlame, code_action_hint::CodeActionHints, inline_completion::InlineCompletions,
};
// Retain document-facing annotation types at their established public paths.
pub use crate::features::inline_completion::{InlineCompletion, InlineCompletionRow};
pub use crate::features::{
    code_lenses::CodeLens,
    document_colors::DocumentColorSwatches,
    document_highlight::DocumentHighlights,
    document_links::DocumentLink,
    document_symbols::{Breadcrumbs, Crumb},
    inlay_hints::{DocumentInlayHints, DocumentInlayHintsId},
};

mod text_io;
pub use text_io::{from_reader, read_to_string, to_writer};
use text_io::{read_file, DecodedFile};

mod syntax_initialization;

use syntax_initialization::PendingSyntax;
pub use syntax_initialization::SyntaxRequest;

use anyhow::{anyhow, bail, Error};
use arc_swap::access::DynAccess;
use arc_swap::ArcSwap;
use editor_core::auto_pairs::AutoPairs;
use editor_core::chars::char_is_word;
use editor_core::diagnostic::DiagnosticProvider;
use editor_core::doc_formatter::TextFormat;
use editor_core::encoding::Encoding;
use editor_core::syntax::config::LanguageServerFeature;
use editor_core::text_annotations::Overlay;
use futures_util::future::BoxFuture;
use lsp_client::util::lsp_pos_to_pos;
use snippets::{ActiveSnippet, SnippetRenderCtx};
use stdx::faccess::{copy_metadata, readonly};
use thiserror;
use vcs::{DiffHandle, DiffProviderRegistry};

use ::parking_lot::Mutex;
use serde::de::{self, Deserialize, Deserializer};
use serde::Serialize;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, OnceLock, Weak};
use std::time::SystemTime;

use editor_core::{
    editor_config::EditorConfig,
    encoding,
    history::History,
    indent::{auto_detect_indent_style, IndentStyle},
    line_ending::auto_detect_line_ending,
    syntax::{self, config::LanguageConfiguration},
    ChangeSet, Diagnostic, LineEnding, Range, Rope, Selection, Syntax, Transaction,
};

use crate::{
    config::Config,
    events::SelectionDidChange,
    features::{
        code_lenses::DocumentCodeLenses, document_colors::DocumentColors,
        document_highlight::DocumentHighlightsState, document_links::DocumentLinks,
        document_symbols::DocumentSymbols, hover::HoverState, inlay_hints::InlayHintsState,
        navigation::NavigationState,
    },
    view::ViewPosition,
    DocumentId, Editor, Theme, View, ViewId,
};

const DEFAULT_INDENT: IndentStyle = IndentStyle::Tabs;
const DEFAULT_TAB_WIDTH: usize = 4;

pub const DEFAULT_LANGUAGE_NAME: &str = "text";

pub const SCRATCH_BUFFER_NAME: &str = "[scratch]";

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Mode {
    Normal = 0,
    Select = 1,
    Insert = 2,
}

impl Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mode::Normal => f.write_str("normal"),
            Mode::Select => f.write_str("select"),
            Mode::Insert => f.write_str("insert"),
        }
    }
}

impl FromStr for Mode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "normal" => Ok(Mode::Normal),
            "select" => Ok(Mode::Select),
            "insert" => Ok(Mode::Insert),
            _ => bail!("Invalid mode '{}'", s),
        }
    }
}

// toml deserializer doesn't seem to recognize string as enum
impl<'de> Deserialize<'de> for Mode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

impl Serialize for Mode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}
/// A snapshot of the text of a document that we want to write out to disk
#[derive(Debug, Clone)]
pub struct DocumentSavedEvent {
    pub revision: usize,
    pub save_time: SystemTime,
    pub doc_id: DocumentId,
    pub path: PathBuf,
    pub text: Rope,
}

pub type DocumentSavedEventResult = Result<DocumentSavedEvent, anyhow::Error>;
pub type DocumentSavedEventFuture = BoxFuture<'static, DocumentSavedEventResult>;

#[derive(Debug)]
pub struct SavePoint {
    /// The view this savepoint is associated with
    pub view: ViewId,
    revert: Mutex<Transaction>,
}

#[derive(Debug, thiserror::Error)]
pub enum DocumentOpenError {
    #[error("path must be a regular file, symlink, or directory")]
    IrregularFile,
    #[error(transparent)]
    IoError(#[from] io::Error),
}

pub struct Document {
    redraw: event::Redraw,
    status: Option<event::status::StatusSender>,
    pub(crate) id: DocumentId,
    text: Rope,
    binary: bool,
    selections: HashMap<ViewId, Selection>,
    view_data: HashMap<ViewId, ViewData>,
    pub active_snippet: Option<ActiveSnippet>,

    /// Jump label overlays for each view.
    pub(crate) jump_labels: HashMap<ViewId, Vec<Overlay>>,

    // Each feature owns its cache, scheduling handle, and request controllers.
    // Dropping the document drops those controllers and cancels in-flight work.
    pub(crate) hover: HoverState,
    pub(crate) navigation: NavigationState,
    pub(crate) inlay_hints: InlayHintsState,
    pub(crate) inline_completions: InlineCompletions,
    pub(crate) document_colors: DocumentColors,
    pub(crate) code_lenses: DocumentCodeLenses,
    pub(crate) document_links: DocumentLinks,
    pub(crate) document_highlights: DocumentHighlightsState,
    pub(crate) code_action_hints: CodeActionHints,
    pub(crate) document_symbols: DocumentSymbols,

    path: Option<PathBuf>,
    relative_path: OnceLock<Option<PathBuf>>,
    display_path: OnceLock<Option<PathBuf>>,
    /// Lazily-computed workspace root for this document (the ancestor that contains a `.git` /
    /// `.svn` / `.jj` / `.mitos`). Avoids per-call `find_workspace_in` ancestor walks for hot
    /// consumers like the statusline trust indicator, LSP launch, and DAP launch. Taken in
    /// `set_path` so save-as recomputes.
    workspace_root: OnceLock<PathBuf>,
    encoding: &'static encoding::Encoding,
    has_bom: bool,

    pub restore_cursor: bool,

    /// Current indent style.
    pub indent_style: IndentStyle,
    editor_config: EditorConfig,
    /// The languages (dictionaries) to spell check this buffer against, or empty to disable spell
    /// checking. A word is flagged only when every dictionary rejects it.
    ///
    /// Resolved by [`Document::detect_spelling_languages`] from the manual override, the
    /// document's `.editorconfig` (`spelling_language = en_US`), or the resolved config.
    pub spelling_languages: Vec<editor_core::SpellingLanguage>,
    /// A manual `:set-spelling-language` choice, which takes precedence over `.editorconfig` and
    /// config when resolving [`Document::spelling_languages`]. `None` means derive from those; an
    /// empty `Vec` forces spell checking off regardless of what they say.
    pub spelling_language_override: Option<Vec<editor_core::SpellingLanguage>>,

    /// The document's default line ending.
    pub line_ending: LineEnding,

    pub syntax: Option<Syntax>,
    pending_syntax: Option<PendingSyntax>,
    // Assigned when the document joins an editor so its events return to that editor.
    pub(crate) signature_help_trigger:
        Option<crate::features::signature_help::SignatureHelpTrigger>,
    pub(crate) word_index_trigger: Option<crate::features::word_index::WordIndexTrigger>,
    pub(crate) auto_save_trigger: Option<crate::features::auto_save::AutoSaveTrigger>,
    pub(crate) syntax_handler: Option<crate::features::syntax::SyntaxHandler>,
    pub(crate) spelling_events:
        Option<tokio::sync::mpsc::Sender<crate::features::spelling::SpellingEvent>>,
    /// Corresponding language scope name. Usually `source.<lang>`.
    pub language: Option<Arc<LanguageConfiguration>>,

    /// Pending changes since last history commit.
    changes: ChangeSet,
    /// Text before the pending changes; cursor snapshots are kept separately.
    pending_original_text: Option<Rope>,
    pub(crate) selection_history: crate::editing::SelectionHistory,
    /// Undo tree.
    // It can be used as a cell where we will take it out to get some parts of the history and put
    // it back as it separated from the edits. We could split out the parts manually but that will
    // be more troublesome.
    pub(crate) history: Cell<History>,
    pub config: Arc<dyn DynAccess<Config>>,

    savepoints: Vec<Weak<SavePoint>>,

    // Last time we wrote to the file. This will carry the time the file was last opened if there
    // were no saves.
    last_saved_time: SystemTime,

    /// On-disk mtime of the last external change auto-reload already surfaced for
    /// this document (prompt/warning). Lets it show the conflict once instead of
    /// re-prompting on every poll/focus check while the buffer stays modified.
    pub auto_reload_seen_mtime: Option<SystemTime>,

    last_saved_revision: usize,
    version: i32, // should be usize?
    pub(crate) modified_since_accessed: bool,

    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) language_servers: HashMap<LanguageServerName, Arc<Client>>,

    diff_handle: Option<DiffHandle>,
    version_control_head: Option<Arc<ArcSwap<Box<str>>>>,
    pub(crate) blame: DocumentBlame,

    // when document was used for most-recent-used buffer picker
    pub focused_at: std::time::Instant,

    pub readonly: bool,

    pub(crate) pull_diagnostics: crate::features::diagnostics::pull::DocumentDiagnostics,

    /// Whether this document owns the startup welcome screen.
    pub is_welcome: bool,

    // NOTE: this field should eventually go away - we should use the Editor's syn_loader instead
    // of storing a copy on every doc. Then we can remove the surrounding `Arc` and use the
    // `ArcSwap` directly.
    syn_loader: Arc<ArcSwap<syntax::Loader>>,
}

use std::fmt;
impl fmt::Debug for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("id", &self.id)
            .field("text", &self.text)
            .field("selections", &self.selections)
            .field("text_annotations", &self.inlay_hints)
            .field("view_data", &self.view_data)
            .field("path", &self.path)
            .field("encoding", &self.encoding)
            .field("restore_cursor", &self.restore_cursor)
            .field("syntax", &self.syntax)
            .field("language", &self.language)
            .field("changes", &self.changes)
            .field("pending_original_text", &self.pending_original_text)
            // .field("history", &self.history)
            .field("last_saved_time", &self.last_saved_time)
            .field("last_saved_revision", &self.last_saved_revision)
            .field("version", &self.version)
            .field("modified_since_accessed", &self.modified_since_accessed)
            .field("diagnostics", &self.diagnostics)
            // .field("language_server", &self.language_server)
            .finish()
    }
}

impl Editor {
    pub(crate) fn clear_doc_relative_paths(&mut self) {
        for doc in self.documents_mut() {
            doc.relative_path.take();
        }
    }
}

use lsp_client::{lsp, Client, LanguageServerId, LanguageServerName};
use stdx::Url;

impl Document {
    pub fn from(
        text: Rope,
        encoding_with_bom_info: Option<(&'static Encoding, bool)>,
        config: Arc<dyn DynAccess<Config>>,
        syn_loader: Arc<ArcSwap<syntax::Loader>>,
    ) -> Self {
        let (encoding, has_bom) = encoding_with_bom_info.unwrap_or((encoding::UTF_8, false));
        let line_ending = config.load().files.default_line_ending.into();
        let changes = ChangeSet::new(text.slice(..));
        let pending_original_text = None;

        Self {
            id: DocumentId::default(),
            active_snippet: None,
            path: None,
            relative_path: OnceLock::new(),
            display_path: OnceLock::new(),
            workspace_root: OnceLock::new(),
            encoding,
            has_bom,
            text,
            binary: false,
            selections: HashMap::default(),
            inlay_hints: Default::default(),
            hover: Default::default(),
            navigation: Default::default(),
            inline_completions: Default::default(),
            view_data: Default::default(),
            indent_style: DEFAULT_INDENT,
            editor_config: EditorConfig::default(),
            spelling_languages: Vec::new(),
            spelling_language_override: None,
            line_ending,
            restore_cursor: false,
            syntax: None,
            pending_syntax: None,
            syntax_handler: None,
            auto_save_trigger: None,
            word_index_trigger: None,
            signature_help_trigger: None,
            spelling_events: None,
            language: None,
            changes,
            pending_original_text,
            selection_history: Default::default(),
            diagnostics: Vec::new(),
            version: 0,
            history: Cell::new(History::default()),
            savepoints: Vec::new(),
            last_saved_time: SystemTime::now(),
            auto_reload_seen_mtime: None,
            last_saved_revision: 0,
            modified_since_accessed: false,
            language_servers: HashMap::new(),
            diff_handle: None,
            redraw: event::Redraw::default(),
            status: None,
            blame: Default::default(),
            config,
            version_control_head: None,
            focused_at: std::time::Instant::now(),
            readonly: false,
            jump_labels: HashMap::new(),
            document_highlights: Default::default(),
            code_action_hints: Default::default(),
            document_links: Default::default(),
            document_colors: Default::default(),
            code_lenses: Default::default(),
            document_symbols: Default::default(),
            syn_loader,
            pull_diagnostics: Default::default(),
            is_welcome: false,
        }
    }

    pub fn default(
        config: Arc<dyn DynAccess<Config>>,
        syn_loader: Arc<ArcSwap<syntax::Loader>>,
    ) -> Self {
        let line_ending: LineEnding = config.load().files.default_line_ending.into();
        let text = Rope::from(line_ending.as_str());
        Self::from(text, None, config, syn_loader)
    }

    #[must_use]
    pub fn with_welcome(mut self) -> Self {
        self.is_welcome = true;
        self
    }

    // TODO: async fn?
    /// Create a new document from `path`. Encoding is auto-detected, but it can be manually
    /// overwritten with the `encoding` parameter.
    /// The editor requests syntax initialization after assigning the document an id.
    pub fn open(
        path: &Path,
        mut encoding: Option<&'static Encoding>,
        detect_language: bool,
        config: Arc<dyn DynAccess<Config>>,
        syn_loader: Arc<ArcSwap<syntax::Loader>>,
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

        let loader = syn_loader.load();
        let mut doc = Self::from(text, Some((encoding, has_bom)), config, syn_loader);

        doc.binary = binary;
        // set the path and try detecting the language
        doc.set_path(Some(path));
        if detect_language {
            // The editor requests syntax after assigning a stable document id.
            doc.language = doc.detect_language_config(&loader);
        }

        doc.editor_config = editor_config;
        doc.detect_indent_and_line_ending();

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

        // futures_util::future::Ready<_>,
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
        if self.binary {
            bail!("Binary files are read-only and cannot be saved as text");
        }
        log::debug!(
            "submitting save of doc '{:?}'",
            self.path().map(|path| path.to_string_lossy())
        );

        // we clone and move text + path into the future so that we asynchronously save the current
        // state without blocking any further edits.
        let text = self.text().clone();

        let path = match path {
            Some(path) => stdx::path::canonicalize(path),
            None => {
                if self.path.is_none() {
                    bail!("Can't save with no path set!");
                }
                self.path.as_ref().unwrap().clone()
            }
        };

        let identifier = self.path().map(|_| self.identifier());
        let language_servers: Vec<_> = self.language_servers.values().cloned().collect();

        // mark changes up to now as saved
        let current_rev = self.get_current_revision();
        let doc_id = self.id();
        let atomic_save = self.config.load().files.save.atomic;

        let encoding_with_bom_info = (self.encoding, self.has_bom);
        let last_saved_time = self.last_saved_time;

        // We encode the file according to the `Document`'s encoding.
        let future = async move {
            use tokio::fs;
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
                to_writer(&mut dst, encoding_with_bom_info, &text).await?;
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
                revision: current_rev,
                save_time,
                doc_id,
                path,
                text: text.clone(),
            };

            for language_server in language_servers {
                if !language_server.is_initialized() {
                    continue;
                }
                if let Some(id) = identifier.clone() {
                    language_server.text_document_did_save(id, &text);
                }
            }

            Ok(event)
        };

        Ok(future)
    }

    /// Detect the language and initialize syntax without delaying file loading.
    pub fn detect_language(&mut self, loader: &Arc<syntax::Loader>) {
        self.hover.clear();
        self.navigation.clear();
        self.document_links.cancel_navigation();
        self.reset_all_inlay_hints();
        self.language = self.detect_language_config(loader);
        self.initialize_syntax(loader.clone());
    }

    /// Detect the programming language based on the file type.
    pub fn detect_language_config(
        &self,
        loader: &syntax::Loader,
    ) -> Option<Arc<syntax::config::LanguageConfiguration>> {
        if self.binary {
            return None;
        }
        let language = loader
            .language_for_filename(self.path.as_ref()?)
            .or_else(|| loader.language_for_shebang(self.text().slice(..)))?;

        Some(loader.language(language).config().clone())
    }

    /// Detect the indentation used in the file, or otherwise defaults to the language indentation
    /// configured in `languages.toml`, with a fallback to tabs if it isn't specified. Line ending
    /// is likewise auto-detected, and will remain unchanged if no line endings were detected.
    pub fn detect_indent_and_line_ending(&mut self) {
        self.indent_style = if let Some(indent_style) = self.editor_config.indent_style {
            indent_style
        } else {
            auto_detect_indent_style(&self.text).unwrap_or_else(|| {
                self.language_config()
                    .and_then(|config| config.indent.as_ref())
                    .map_or(DEFAULT_INDENT, |config| IndentStyle::from_str(&config.unit))
            })
        };
        if let Some(line_ending) = self
            .editor_config
            .line_ending
            .or_else(|| auto_detect_line_ending(&self.text))
        {
            self.line_ending = line_ending;
        }
        self.detect_spelling_languages();
    }

    /// Resolves the languages this document is spell-checked against, in precedence order: a manual
    /// `:set-spelling-language` override, then the `.editorconfig` `spelling_language`, then the
    /// resolved config (`languages.toml` over `[editor.spelling]`). Re-run when any of these change.
    pub fn detect_spelling_languages(&mut self) {
        if self.binary {
            self.spelling_languages.clear();
            return;
        }
        let config = self.config.load();
        self.spelling_languages = if let Some(languages) = &self.spelling_language_override {
            languages.clone()
        } else if let Some(language) = self
            .editor_config
            .spelling_language
            .as_ref()
            .filter(|_| config.files.editor_config)
        {
            vec![language.clone()]
        } else {
            config
                .spelling
                .merged(
                    self.language_config()
                        .and_then(|config| config.spelling.as_ref()),
                )
                .languages()
                .to_vec()
        };
        let mut seen = HashSet::new();
        self.spelling_languages
            .retain(|language| seen.insert(language.clone()));
    }

    pub fn detect_editor_config(&mut self) {
        if self.config.load().files.editor_config
            && let Some(path) = self.path.as_ref()
        {
            self.editor_config = EditorConfig::find(path);
        }
    }

    /// Binary files have no editable text; their original bytes remain on disk.
    pub fn is_binary(&self) -> bool {
        self.binary
    }

    /// Filesystem modification time recorded by the last read or successful save.
    pub fn last_saved_time(&self) -> SystemTime {
        self.last_saved_time
    }

    pub fn pickup_last_saved_time(&mut self) {
        self.auto_reload_seen_mtime = None;
        self.last_saved_time = match self.path() {
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
        self.readonly = self.binary || self.path.as_ref().is_some_and(|p| readonly(p));
    }

    /// Reload the document from its path.
    pub fn reload(
        &mut self,
        provider_registry: &DiffProviderRegistry,
        trust_full: bool,
    ) -> Result<(), Error> {
        let encoding = self.encoding;
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
        // Reload may change the content type. Allow this replacement, then restore
        // the binary write guard before returning to the editor.
        let was_binary = std::mem::replace(&mut self.binary, false);

        // Calculate the difference between the buffer and source text, and apply it.
        // This is not considered a modification of the contents of the file regardless
        // of the encoding.
        let transaction = editor_core::diff::compare_ropes(self.text(), &rope);
        self.apply(transaction.changes())?;
        self.commit_changes();
        self.binary = binary;
        self.detect_readonly();
        if binary {
            self.hover.clear();
            self.navigation.clear();
            self.document_links.cancel_navigation();
            self.reset_all_inlay_hints();
            self.language = None;
            self.syntax = None;
            self.pending_syntax = None;
            self.language_servers.clear();
        } else if was_binary {
            let loader = self.syn_loader.load_full();
            self.detect_language(&loader);
        }
        self.reset_modified();
        self.pickup_last_saved_time();
        self.detect_indent_and_line_ending();

        self.refresh_vcs(provider_registry, trust_full);

        Ok(())
    }

    /// Sets the [`Document`]'s encoding with the encoding correspondent to `label`.
    pub fn set_encoding(&mut self, label: &str) -> Result<(), Error> {
        let encoding =
            Encoding::for_label(label.as_bytes()).ok_or_else(|| anyhow!("unknown encoding"))?;

        self.encoding = encoding;

        Ok(())
    }

    /// Returns the [`Document`]'s current encoding.
    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }

    /// sets the document path without sending events to various
    /// observers (like LSP), in most cases `Editor::set_doc_path`
    /// should be used instead
    pub fn set_path(&mut self, path: Option<&Path>) {
        self.invalidate_blame();
        self.hover.clear();
        self.navigation.clear();
        self.document_links.cancel_navigation();
        self.reset_all_inlay_hints();
        let path = path.map(stdx::path::canonicalize);

        // `take` to remove any prior relative path that may have existed.
        // This will get set in `relative_path()`.
        self.relative_path.take();
        self.display_path.take();
        // Same story: invalidate so the next workspace_root() recomputes against the new path.
        self.workspace_root.take();

        // if parent doesn't exist we still want to open the document
        // and error out when document is saved
        self.path = path;

        self.detect_readonly();
        self.pickup_last_saved_time();
    }

    /// Set the language and load its syntax immediately for an explicit language command.
    /// Automatic file loading uses [`Self::detect_language`] to initialize in the background.
    pub fn set_language(
        &mut self,
        language_config: Option<Arc<LanguageConfiguration>>,
        loader: &syntax::Loader,
    ) {
        self.pending_syntax = None;
        self.hover.clear();
        self.navigation.clear();
        self.document_links.cancel_navigation();
        self.reset_all_inlay_hints();
        self.language = language_config;
        self.syntax = self.language.as_ref().and_then(|config| {
            Syntax::new(self.text.slice(..), config.language(), loader)
                .map_err(|err| {
                    if err != syntax::HighlighterError::NoRootConfig {
                        log::warn!("Error building syntax for '{}': {err}", self.display_name());
                    }
                })
                .ok()
        });
    }

    /// Set the programming language for the file if you know the language but don't have the
    /// [`syntax::config::LanguageConfiguration`] for it.
    pub fn set_language_by_language_id(
        &mut self,
        language_id: &str,
        loader: &syntax::Loader,
    ) -> anyhow::Result<()> {
        let language = loader
            .language_for_name(language_id)
            .ok_or_else(|| anyhow!("invalid language id: {}", language_id))?;
        let config = loader.language(language).config().clone();
        self.set_language(Some(config), loader);
        Ok(())
    }

    /// Select text within the [`Document`].
    pub fn set_selection(&mut self, view_id: ViewId, selection: Selection) {
        // TODO: use a transaction?
        self.update_selection(view_id, selection);
        let errors = event::dispatch(SelectionDidChange {
            doc: self,
            view: view_id,
        });
        self.report_hook_errors(errors)
    }

    /// Find the origin selection of the text in a document, i.e. where
    /// a single cursor would go if it were on the first grapheme. If
    /// the text is empty, returns (0, 0).
    pub fn origin(&self) -> Range {
        if self.text().len_chars() == 0 {
            return Range::new(0, 0);
        }

        Range::new(0, 1).grapheme_aligned(self.text().slice(..))
    }

    /// Get the line of cursor for the primary selection
    pub fn cursor_line(&self, view_id: ViewId) -> usize {
        let text = self.text();
        let selection = self.selection(view_id);
        text.char_to_line(selection.primary().cursor(text.slice(..)))
    }

    /// Reset the view's selection on this document to the
    /// [origin](Document::origin) cursor.
    pub fn reset_selection(&mut self, view_id: ViewId) {
        let origin = self.origin();
        self.set_selection(view_id, Selection::single(origin.anchor, origin.head));
    }

    /// Initializes a new selection and view_data for the given view
    /// if it does not already have them.
    pub fn ensure_view_init(&mut self, view_id: ViewId) {
        if !self.selections.contains_key(&view_id) {
            self.reset_selection(view_id);
        }

        self.view_data_mut(view_id);

        if self.breadcrumb_enabled() {
            self.update_breadcrumbs_for_view(view_id);
        }
    }

    /// Mark document as recent used for MRU sorting
    pub fn mark_as_focused(&mut self) {
        self.focused_at = std::time::Instant::now();
    }

    /// Remove any views' data and state that is stored in the `Document`.
    pub fn remove_view(&mut self, view_id: ViewId) {
        self.selections.remove(&view_id);
        self.view_data.remove(&view_id);
        self.inlay_hints.remove_view(view_id);
        self.hover.remove_view(view_id);
        self.navigation.remove_view(view_id);
        self.document_links.cancel_navigation_for(view_id);
        self.jump_labels.remove(&view_id);
        self.document_symbols.remove_view(view_id);
        self.document_highlights.remove_view(view_id);
        self.inline_completions.remove_view(self.id, view_id);
        self.code_action_hints.remove_view(view_id);
    }

    /// Creates a reference counted snapshot (called savpepoint) of the document.
    ///
    /// The snapshot will remain valid (and updated) idenfinitly as long as ereferences to it exist.
    /// Restoring the snapshot will restore the selection and the contents of the document to
    /// the state it had when this function was called.
    pub fn savepoint(&mut self, view: &View) -> Arc<SavePoint> {
        let revert = Transaction::new(self.text()).with_selection(self.selection(view.id).clone());
        // check if there is already an existing (identical) savepoint around
        if let Some(savepoint) = self
            .savepoints
            .iter()
            .rev()
            .find_map(|savepoint| savepoint.upgrade())
        {
            let transaction = savepoint.revert.lock();
            if savepoint.view == view.id
                && transaction.changes().is_empty()
                && transaction.selection() == revert.selection()
            {
                drop(transaction);
                return savepoint;
            }
        }
        let savepoint = Arc::new(SavePoint {
            view: view.id,
            revert: Mutex::new(revert),
        });
        self.savepoints.push(Arc::downgrade(&savepoint));
        savepoint
    }

    pub fn restore(&mut self, view: &mut View, savepoint: &SavePoint, emit_lsp_notification: bool) {
        assert_eq!(
            savepoint.view, view.id,
            "Savepoint must not be used with a different view!"
        );
        // search and remove savepoint using a ptr comparison
        // this avoids a deadlock as we need to lock the mutex
        let savepoint_idx = self
            .savepoints
            .iter()
            .position(|savepoint_ref| std::ptr::eq(savepoint_ref.as_ptr(), savepoint))
            .expect("Savepoint must belong to this document");

        let savepoint_ref = self.savepoints.remove(savepoint_idx);
        let mut revert = savepoint.revert.lock();
        crate::editing::apply_with_notifications(self, &revert, view.id, emit_lsp_notification);
        *revert = Transaction::new(self.text()).with_selection(self.selection(view.id).clone());
        self.savepoints.push(savepoint_ref)
    }

    pub fn id(&self) -> DocumentId {
        self.id
    }

    /// If there are unsaved modifications.
    pub fn is_modified(&self) -> bool {
        let history = self.history.take();
        let current_revision = history.current_revision();
        self.history.set(history);
        log::debug!(
            "id {} modified - last saved: {}, current: {}",
            self.id,
            self.last_saved_revision,
            current_revision
        );
        current_revision != self.last_saved_revision || !self.changes.is_empty()
    }

    /// Save modifications to history, and so [`Self::is_modified`] will return false.
    pub fn reset_modified(&mut self) {
        let history = self.history.take();
        let current_revision = history.current_revision();
        self.history.set(history);
        self.last_saved_revision = current_revision;
    }

    /// Set the document's latest saved revision to the given one.
    pub fn set_last_saved_revision(&mut self, rev: usize, save_time: SystemTime) {
        log::debug!(
            "doc {} revision updated {} -> {}",
            self.id,
            self.last_saved_revision,
            rev
        );
        self.last_saved_revision = rev;
        self.last_saved_time = save_time;
        self.auto_reload_seen_mtime = None;
    }

    /// Get the document's latest saved revision.
    pub fn get_last_saved_revision(&mut self) -> usize {
        self.last_saved_revision
    }

    /// Get the current revision number
    pub fn get_current_revision(&mut self) -> usize {
        let history = self.history.take();
        let current_revision = history.current_revision();
        self.history.set(history);
        current_revision
    }

    /// Corresponding language scope name. Usually `source.<lang>`.
    pub fn language_scope(&self) -> Option<&str> {
        self.language
            .as_ref()
            .map(|language| language.scope.as_str())
    }

    /// Language name for the document. Corresponds to the `name` key in
    /// `languages.toml` configuration.
    pub fn language_name(&self) -> Option<&str> {
        self.language
            .as_ref()
            .map(|language| language.language_id.as_str())
    }

    /// Language ID for the document. Either the `language-id`,
    /// or the document language name if no `language-id` has been specified.
    pub fn language_id(&self) -> Option<&str> {
        self.language_config()?
            .language_server_language_id
            .as_deref()
            .or_else(|| self.language_name())
    }

    /// Corresponding [`LanguageConfiguration`].
    pub fn language_config(&self) -> Option<&LanguageConfiguration> {
        self.language.as_deref()
    }

    /// The language configuration of the injection layer at `byte_pos`,
    /// so language-specific behavior follows embedded languages. Falls back to the
    /// document's root language config when there is no syntax tree.
    pub fn language_config_at<'a>(
        &'a self,
        loader: &'a syntax::Loader,
        byte_pos: usize,
    ) -> Option<&'a LanguageConfiguration> {
        match self.syntax() {
            Some(syntax) => {
                let layer = syntax.layer_for_byte_range(byte_pos as u32, byte_pos as u32);
                Some(&**loader.language(syntax.layer(layer).language).config())
            }
            None => self.language_config(),
        }
    }

    /// Current document version, incremented at each change.
    pub fn version(&self) -> i32 {
        self.version
    }

    pub fn word_completion_enabled(&self) -> bool {
        self.language_config()
            .and_then(|lang_config| lang_config.word_completion.and_then(|config| config.enable))
            .unwrap_or_else(|| self.config.load().completion.word.enable)
    }

    pub fn path_completion_enabled(&self) -> bool {
        self.language_config()
            .and_then(|lang_config| lang_config.path_completion)
            .unwrap_or_else(|| self.config.load().completion.path.enable)
    }

    /// maintains the order as configured in the language_servers TOML array
    pub fn language_servers(&self) -> impl Iterator<Item = &lsp_client::Client> {
        self.language_config().into_iter().flat_map(move |config| {
            config.language_servers.iter().filter_map(move |features| {
                let ls = &**self.language_servers.get(&features.name)?;
                if ls.is_initialized() {
                    Some(ls)
                } else {
                    None
                }
            })
        })
    }

    pub fn remove_language_server_by_name(&mut self, name: &str) -> Option<Arc<Client>> {
        let server = self.language_servers.remove(name);
        if server.is_some() {
            self.hover.clear();
            self.navigation.clear();
            self.document_links.cancel_navigation();
            self.reset_all_inlay_hints();
        }
        server
    }

    pub fn language_servers_with_feature(
        &self,
        feature: LanguageServerFeature,
    ) -> impl Iterator<Item = &lsp_client::Client> {
        self.language_config().into_iter().flat_map(move |config| {
            config.language_servers.iter().filter_map(move |features| {
                let ls = &**self.language_servers.get(&features.name)?;
                if ls.is_initialized()
                    && ls.supports_feature(feature)
                    && features.has_feature(feature)
                {
                    Some(ls)
                } else {
                    None
                }
            })
        })
    }

    pub fn supports_language_server(&self, id: LanguageServerId) -> bool {
        self.language_servers().any(|l| l.id() == id)
    }

    pub fn servers_to_load(&self) -> bool {
        self.language_config()
            .map(|lang| !lang.language_servers.is_empty() || lang.debugger.is_some())
            .unwrap_or(false)
    }

    /// Signal destination for standalone documents; attached documents use their editor's.
    pub fn redraw(&self) -> &event::Redraw {
        &self.redraw
    }

    pub(crate) fn bind_signals(
        &mut self,
        redraw: event::Redraw,
        status: event::status::StatusSender,
    ) {
        self.redraw = redraw;
        self.status = Some(status);
    }

    fn report_hook_errors(&self, errors: event::HookErrors) {
        if let Some(status) = &self.status {
            errors.report_to(status);
        } else {
            errors.log();
        }
    }

    pub fn diff_handle(&self) -> Option<&DiffHandle> {
        self.diff_handle.as_ref()
    }

    /// Refresh both branch display and the diff base after repository changes.
    pub fn refresh_vcs(&mut self, providers: &DiffProviderRegistry, trust_full: bool) {
        self.update_blame_trust(trust_full);
        self.blame.refresh();
        let Some(path) = self.path().map(ToOwned::to_owned) else {
            return;
        };
        if self.binary {
            self.diff_handle = None;
        } else {
            match providers.get_diff_base(&path, trust_full) {
                Some(base) => self.set_diff_base(base),
                None => self.diff_handle = None,
            }
        }
        self.version_control_head = providers.get_current_head_name(&path, trust_full);
        if self.config.load().inline_blame.show != crate::config::InlineBlameShow::Never
            && let Some(handler) = self.blame.handler().cloned()
        {
            handler.schedule_refresh(self.id());
        }
    }

    /// Initialize or update the differ for this document with a new base.
    pub fn set_diff_base(&mut self, diff_base: Vec<u8>) {
        if self.binary {
            self.diff_handle = None;
            return;
        }
        match from_reader(&mut diff_base.as_slice(), Some(self.encoding)) {
            Ok((diff_base, ..)) => {
                if let Some(differ) = &self.diff_handle {
                    differ.update_diff_base(diff_base);
                    return;
                }
                self.diff_handle = Some(DiffHandle::new(
                    diff_base,
                    self.text.clone(),
                    self.redraw.clone(),
                ))
            }
            _ => {
                self.diff_handle = None;
            }
        }
    }

    pub fn version_control_head(&self) -> Option<Arc<Box<str>>> {
        self.version_control_head.as_ref().map(|a| a.load_full())
    }

    pub fn set_version_control_head(
        &mut self,
        version_control_head: Option<Arc<ArcSwap<Box<str>>>>,
    ) {
        self.version_control_head = version_control_head;
    }

    #[inline]
    /// Tree-sitter AST tree
    pub fn syntax(&self) -> Option<&Syntax> {
        self.syntax.as_ref()
    }

    /// The width that the tab character is rendered at
    pub fn tab_width(&self) -> usize {
        self.editor_config
            .tab_width
            .map(|n| n.get() as usize)
            .unwrap_or_else(|| {
                self.language_config()
                    .and_then(|config| config.indent.as_ref())
                    .map_or(DEFAULT_TAB_WIDTH, |config| config.tab_width)
            })
    }

    // The width (in spaces) of a level of indentation.
    pub fn indent_width(&self) -> usize {
        self.indent_style.indent_width(self.tab_width())
    }

    /// Whether the document should have a trailing line ending appended on save.
    pub fn insert_final_newline(&self) -> bool {
        self.editor_config
            .insert_final_newline
            .unwrap_or_else(|| self.config.load().files.save.insert_final_newline)
    }

    /// Whether the document should trim whitespace preceding line endings on save.
    pub fn trim_trailing_whitespace(&self) -> bool {
        self.editor_config
            .trim_trailing_whitespace
            .unwrap_or_else(|| self.config.load().files.save.trim_trailing_whitespace)
    }

    pub fn changes(&self) -> &ChangeSet {
        &self.changes
    }

    #[inline]
    /// File path on disk.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether this file-backed document should display and maintain breadcrumbs.
    #[inline]
    pub fn breadcrumb_enabled(&self) -> bool {
        self.path.is_some() && self.config.load().breadcrumb.enable
    }

    /// File path as a URL.
    pub fn url(&self) -> Option<Url> {
        Url::from_file_path(self.path()?).ok()
    }

    pub fn uri(&self) -> Option<editor_core::Uri> {
        Some(self.path()?.into())
    }

    #[inline]
    pub fn text(&self) -> &Rope {
        &self.text
    }

    #[inline]
    pub fn selection(&self, view_id: ViewId) -> &Selection {
        &self.selections[&view_id]
    }

    #[inline]
    pub fn selections(&self) -> &HashMap<ViewId, Selection> {
        &self.selections
    }

    fn view_data(&self, view_id: ViewId) -> &ViewData {
        self.view_data
            .get(&view_id)
            .expect("This should only be called after ensure_view_init")
    }

    fn view_data_mut(&mut self, view_id: ViewId) -> &mut ViewData {
        self.view_data.entry(view_id).or_default()
    }

    pub(crate) fn get_view_offset(&self, view_id: ViewId) -> Option<ViewPosition> {
        Some(self.view_data.get(&view_id)?.view_position)
    }

    pub fn view_offset(&self, view_id: ViewId) -> ViewPosition {
        self.view_data(view_id).view_position
    }

    pub fn set_view_offset(&mut self, view_id: ViewId, new_offset: ViewPosition) {
        self.view_data_mut(view_id).view_position = new_offset;
    }

    pub fn relative_path(&self) -> Option<&Path> {
        self.relative_path
            .get_or_init(|| {
                self.path
                    .as_ref()
                    .map(|path| stdx::path::get_relative_path(path).to_path_buf())
            })
            .as_deref()
    }

    /// The workspace root for this document — the nearest ancestor that contains a `.git`, `.svn`,
    /// `.jj`, or `.mitos`. Falls back to the current working directory's workspace when the
    /// document has no path (scratch buffers). Lazily memoised on first call.
    pub fn workspace_root(&self) -> &Path {
        self.workspace_root
            .get_or_init(|| match self.path.as_deref() {
                Some(p) => p
                    .parent()
                    .map(|dir| loader::find_workspace_in(dir).0)
                    .unwrap_or_else(|| loader::find_workspace().0),
                None => loader::find_workspace().0,
            })
    }

    /// A stable path for user-facing output, relative to this document's workspace when possible.
    pub fn display_path(&self) -> Option<&Path> {
        self.display_path
            .get_or_init(|| {
                self.path.as_deref().map(|path| {
                    stdx::path::get_relative_path_from(path, self.workspace_root()).into_owned()
                })
            })
            .as_deref()
    }

    pub fn display_name(&self) -> Cow<'_, str> {
        self.display_path()
            .map_or_else(|| SCRATCH_BUFFER_NAME.into(), |path| path.to_string_lossy())
    }

    // transact(Fn) ?

    // -- LSP methods

    #[inline]
    pub fn identifier(&self) -> lsp::TextDocumentIdentifier {
        lsp::TextDocumentIdentifier::new(self.url().unwrap())
    }

    pub fn versioned_identifier(&self) -> lsp::VersionedTextDocumentIdentifier {
        lsp::VersionedTextDocumentIdentifier::new(self.url().unwrap(), self.version)
    }

    pub fn position(
        &self,
        view_id: ViewId,
        offset_encoding: lsp_client::OffsetEncoding,
    ) -> lsp::Position {
        let text = self.text();

        lsp_client::util::pos_to_lsp_pos(
            text,
            self.selection(view_id).primary().cursor(text.slice(..)),
            offset_encoding,
        )
    }

    pub fn lsp_diagnostic_to_diagnostic(
        text: &Rope,
        language_config: Option<&LanguageConfiguration>,
        diagnostic: &lsp_client::lsp::Diagnostic,
        provider: DiagnosticProvider,
        offset_encoding: lsp_client::OffsetEncoding,
    ) -> Option<Diagnostic> {
        use editor_core::diagnostic::{Range, Severity::*};

        // TODO: convert inside server
        let start =
            if let Some(start) = lsp_pos_to_pos(text, diagnostic.range.start, offset_encoding) {
                start
            } else {
                log::warn!("lsp position out of bounds - {:?}", diagnostic);
                return None;
            };

        let end = if let Some(end) = lsp_pos_to_pos(text, diagnostic.range.end, offset_encoding) {
            end
        } else {
            log::warn!("lsp position out of bounds - {:?}", diagnostic);
            return None;
        };

        let severity = diagnostic.severity.and_then(|severity| match severity {
            lsp::DiagnosticSeverity::ERROR => Some(Error),
            lsp::DiagnosticSeverity::WARNING => Some(Warning),
            lsp::DiagnosticSeverity::INFORMATION => Some(Info),
            lsp::DiagnosticSeverity::HINT => Some(Hint),
            severity => {
                log::error!("unrecognized diagnostic severity: {:?}", severity);
                None
            }
        });

        if let Some(lang_conf) = language_config
            && let Some(severity) = severity
            && severity < lang_conf.diagnostic_severity
        {
            return None;
        };
        use editor_core::diagnostic::{DiagnosticTag, NumberOrString};

        let code = match diagnostic.code.clone() {
            Some(x) => match x {
                lsp::NumberOrString::Number(x) => Some(NumberOrString::Number(x)),
                lsp::NumberOrString::String(x) => Some(NumberOrString::String(x.into())),
            },
            None => None,
        };

        let tags = if let Some(tags) = &diagnostic.tags {
            tags.iter()
                .filter_map(|tag| match *tag {
                    lsp::DiagnosticTag::DEPRECATED => Some(DiagnosticTag::Deprecated),
                    lsp::DiagnosticTag::UNNECESSARY => Some(DiagnosticTag::Unnecessary),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };

        let ends_at_word =
            start != end && end != 0 && text.get_char(end - 1).is_some_and(char_is_word);
        let starts_at_word = start != end && text.get_char(start).is_some_and(char_is_word);

        Some(Diagnostic {
            range: Range { start, end },
            ends_at_word,
            starts_at_word,
            zero_width: start == end,
            line: diagnostic.range.start.line as usize,
            message: diagnostic.message.clone().into(),
            severity,
            code,
            tags,
            source: diagnostic.source.clone().map(|source| source.into()),
            data: diagnostic.data.clone(),
            provider,
        })
    }

    #[inline]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn replace_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        unchanged_sources: &[String],
        provider: Option<&DiagnosticProvider>,
    ) {
        if unchanged_sources.is_empty() {
            if let Some(provider) = provider {
                self.diagnostics
                    .retain(|diagnostic| &diagnostic.provider != provider);
            } else {
                self.diagnostics.clear();
            }
        } else {
            self.diagnostics.retain(|d| {
                if provider.is_some_and(|provider| provider != &d.provider) {
                    return true;
                }

                d.source
                    .as_ref()
                    .is_some_and(|source| unchanged_sources.iter().any(|s| **s == **source))
            });
        }
        self.extend_diagnostics(diagnostics);
    }

    /// Refresh cached language-server diagnostics without discarding diagnostics
    /// owned by other providers.
    pub fn replace_language_server_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
    ) {
        self.diagnostics
            .retain(|diagnostic| !matches!(diagnostic.provider, DiagnosticProvider::Lsp { .. }));
        self.extend_diagnostics(diagnostics);
    }

    fn extend_diagnostics(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diagnostics);
        self.diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range,
                diagnostic.severity,
                diagnostic.provider.clone(),
            )
        });
    }

    /// Replaces `provider`'s diagnostics that fall within any of `regions` with `diagnostics`,
    /// leaving every other diagnostic untouched, including `provider`'s own diagnostics that lie
    /// outside `regions`.
    ///
    /// This is the incremental counterpart to [`Document::replace_diagnostics`]: when only bounded
    /// regions of the document have been re-examined (e.g. the areas around an edit), the caller
    /// re-checks those regions and splices the results in, rather than recomputing the whole
    /// document. Diagnostics outside `regions` keep the positions they were edit-mapped to.
    ///
    /// `regions` are half-open char ranges in the current document text; `diagnostics` are expected
    /// to fall within them. A diagnostic is considered inside a region when their ranges overlap.
    pub fn splice_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        regions: &[std::ops::Range<usize>],
        provider: &DiagnosticProvider,
    ) {
        self.diagnostics.retain(|d| {
            &d.provider != provider
                || !regions
                    .iter()
                    .any(|region| d.range.start < region.end && region.start < d.range.end)
        });
        self.extend_diagnostics(diagnostics);
    }

    /// clears diagnostics for a given language server id if set, otherwise all diagnostics are cleared
    pub fn clear_diagnostics_for_language_server(&mut self, id: LanguageServerId) {
        self.diagnostics
            .retain(|d| d.provider.language_server_id() != Some(id));
    }

    /// Get the document's auto pairs. If the document has a recognized
    /// language config with auto pairs configured, returns that;
    /// otherwise, falls back to the global auto pairs config. If the global
    /// config is false, then ignore language settings.
    pub fn auto_pairs<'a>(
        &'a self,
        editor: &'a Editor,
        loader: &'a syntax::Loader,
        view: &View,
    ) -> Option<&'a AutoPairs> {
        let global_config = (editor.auto_pairs).as_ref();

        // NOTE: If the user specifies the global auto pairs config as false, then
        //       we want to disable it globally regardless of language settings
        #[allow(clippy::question_mark)]
        {
            if global_config.is_none() {
                return None;
            }
        }

        self.syntax
            .as_ref()
            .and_then(|syntax| {
                let selection = self.selection(view.id).primary();
                let (start, end) = selection.into_byte_range(self.text().slice(..));
                let layer = syntax.layer_for_byte_range(start as u32, end as u32);

                let lang_config = loader.language(syntax.layer(layer).language).config();
                lang_config.auto_pairs.as_ref()
            })
            .or(global_config)
    }

    pub fn snippet_ctx(&self) -> SnippetRenderCtx {
        SnippetRenderCtx {
            // TODO snippet variable resolution
            resolve_var: Box::new(|_| None),
            tab_width: self.tab_width(),
            indent_style: self.indent_style,
            line_ending: self.line_ending.as_str(),
        }
    }

    pub fn text_width(&self) -> usize {
        self.editor_config
            .max_line_length
            .map(|n| n.get() as usize)
            .or_else(|| self.language_config().and_then(|config| config.text_width))
            .unwrap_or_else(|| self.config.load().text_width)
    }

    pub fn text_format(&self, mut viewport_width: u16, theme: Option<&Theme>) -> TextFormat {
        let config = self.config.load();
        let text_width = self.text_width();
        let mut soft_wrap_at_text_width = self
            .language_config()
            .and_then(|config| {
                config
                    .soft_wrap
                    .as_ref()
                    .and_then(|soft_wrap| soft_wrap.wrap_at_text_width)
            })
            .or(config.soft_wrap.wrap_at_text_width)
            .unwrap_or(false);
        if soft_wrap_at_text_width {
            // if the viewport is smaller than the specified
            // width then this setting has no effcet
            if text_width >= viewport_width as usize {
                soft_wrap_at_text_width = false;
            } else {
                viewport_width = text_width as u16;
            }
        }
        let config = self.config.load();
        let editor_soft_wrap = &config.soft_wrap;
        let language_soft_wrap = self
            .language
            .as_ref()
            .and_then(|config| config.soft_wrap.as_ref());
        let enable_soft_wrap = language_soft_wrap
            .and_then(|soft_wrap| soft_wrap.enable)
            .or(editor_soft_wrap.enable)
            .unwrap_or(false);
        let max_wrap = language_soft_wrap
            .and_then(|soft_wrap| soft_wrap.max_wrap)
            .or(config.soft_wrap.max_wrap)
            .unwrap_or(20);
        let max_indent_retain = language_soft_wrap
            .and_then(|soft_wrap| soft_wrap.max_indent_retain)
            .or(editor_soft_wrap.max_indent_retain)
            .unwrap_or(40);
        let tab_width = self.tab_width() as u16;
        TextFormat {
            soft_wrap: enable_soft_wrap && viewport_width > 10,
            tab_width,
            max_wrap: max_wrap.min(viewport_width / 4),
            max_indent_retain: max_indent_retain.min(viewport_width * 2 / 5),
            wrap_indicator: Box::from(""),
            // avoid spinning forever when the window manager
            // sets the size to something tiny
            viewport_width,
            wrap_indicator_highlight: theme
                .and_then(|theme| theme.find_highlight("ui.virtual.wrap")),
            soft_wrap_at_text_width,
        }
    }

    /// Set the inlay hints for this document and `view_id`.
    pub fn set_inlay_hints(&mut self, view_id: ViewId, inlay_hints: DocumentInlayHints) {
        self.inlay_hints.set(view_id, inlay_hints);
    }

    pub fn set_jump_labels(&mut self, view_id: ViewId, labels: Vec<Overlay>) {
        self.jump_labels.insert(view_id, labels);
    }

    pub fn remove_jump_labels(&mut self, view_id: ViewId) {
        self.jump_labels.remove(&view_id);
    }

    /// Get the inlay hints for this document and `view_id`.
    pub fn inlay_hints(&self, view_id: ViewId) -> Option<&DocumentInlayHints> {
        self.inlay_hints.get(view_id)
    }

    /// Completely removes all the inlay hints saved for the document, dropping them to free memory
    /// (since it often means inlay hints have been fully deactivated).
    pub fn reset_all_inlay_hints(&mut self) {
        self.inlay_hints.clear();
    }

    pub fn has_language_server_with_feature(&self, feature: LanguageServerFeature) -> bool {
        self.language_servers_with_feature(feature).next().is_some()
    }
}

#[derive(Debug, Default)]
pub struct ViewData {
    view_position: ViewPosition,
}

#[cfg(test)]
mod test {
    use arc_swap::ArcSwap;

    use super::*;

    fn feature_document() -> Document {
        Document::from(
            Rope::from("sample text"),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        )
    }

    #[tokio::test]
    async fn removing_a_view_cancels_only_its_feature_requests() {
        let mut doc = feature_document();
        let mut views = slotmap::SlotMap::<ViewId, ()>::with_key();
        let first = views.insert(());
        let second = views.insert(());
        doc.set_document_highlights(first, std::iter::once(0..2).collect());
        doc.set_document_highlights(second, std::iter::once(3..5).collect());
        doc.set_code_action_hints(first);
        doc.set_code_action_hints(second);
        let first_highlight = doc.document_highlight_controller(first).restart();
        let second_highlight = doc.document_highlight_controller(second).restart();
        let first_hint = doc.code_action_controller(first).restart();
        let second_hint = doc.code_action_controller(second).restart();

        doc.remove_view(first);

        assert!(doc.document_highlights(first).is_none());
        assert!(!doc.code_action_hints(first));
        assert!(first_highlight.is_canceled());
        assert!(first_hint.is_canceled());
        let remaining = doc.document_highlights(second).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0], 3..5);
        assert!(doc.code_action_hints(second));
        assert!(!second_highlight.is_canceled());
        assert!(!second_hint.is_canceled());

        doc.clear_all_document_highlights();
        doc.clear_all_code_action_hints();
        assert!(doc.document_highlights(second).is_none());
        assert!(!doc.code_action_hints(second));
        assert!(second_highlight.is_canceled());
        assert!(second_hint.is_canceled());
    }

    #[tokio::test]
    async fn clearing_render_caches_keeps_replacement_requests_alive() {
        let mut doc = feature_document();
        let view = ViewId::default();
        doc.set_document_highlights(view, std::iter::once(0..2).collect());
        let highlight = doc.document_highlight_controller(view).restart();

        doc.clear_document_highlights(view);

        assert!(doc.document_highlights(view).is_none());
        assert!(!highlight.is_canceled());

        // Code-action invalidation intentionally cancels the current request.
        let hint = doc.code_action_controller(view).restart();
        doc.clear_code_action_hints(view);
        assert!(hint.is_canceled());
    }

    #[tokio::test]
    async fn dropping_a_document_cancels_view_feature_requests() {
        let mut doc = feature_document();
        let view = ViewId::default();
        let requests = [
            doc.document_highlight_controller(view).restart(),
            doc.code_action_controller(view).restart(),
        ];
        drop(doc);
        assert!(requests.iter().all(event::TaskHandle::is_canceled));
    }

    #[tokio::test]
    async fn changeset_to_changes_ignore_line_endings() {
        use lsp_client::{lsp, Client, OffsetEncoding};
        let text = Rope::from("hello\r\nworld");
        let mut doc = Document::from(
            text,
            None,
            Arc::new(ArcSwap::new(Arc::new(Config::default()))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let view = ViewId::default();
        doc.set_selection(view, Selection::single(0, 0));

        let transaction =
            Transaction::change(doc.text(), vec![(5, 7, Some("\n".into()))].into_iter());
        let old_doc = doc.text().clone();
        crate::editing::apply(&mut doc, &transaction, view);
        let changes = Client::changeset_to_changes(
            &old_doc,
            doc.text(),
            transaction.changes(),
            OffsetEncoding::Utf8,
        );

        assert_eq!(doc.text(), "hello\nworld");

        assert_eq!(
            changes,
            &[lsp::TextDocumentContentChangeEvent {
                range: Some(lsp::Range::new(
                    lsp::Position::new(0, 5),
                    lsp::Position::new(1, 0)
                )),
                text: "\n".into(),
                range_length: None,
            }]
        );
    }

    #[tokio::test]
    async fn changeset_to_changes() {
        use lsp_client::{lsp, Client, OffsetEncoding};
        let text = Rope::from("hello");
        let mut doc = Document::from(
            text,
            None,
            Arc::new(ArcSwap::new(Arc::new(Config::default()))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let view = ViewId::default();
        doc.set_selection(view, Selection::single(5, 5));

        // insert

        let transaction = Transaction::insert(doc.text(), doc.selection(view), " world".into());
        let old_doc = doc.text().clone();
        crate::editing::apply(&mut doc, &transaction, view);
        let changes = Client::changeset_to_changes(
            &old_doc,
            doc.text(),
            transaction.changes(),
            OffsetEncoding::Utf8,
        );

        assert_eq!(
            changes,
            &[lsp::TextDocumentContentChangeEvent {
                range: Some(lsp::Range::new(
                    lsp::Position::new(0, 5),
                    lsp::Position::new(0, 5)
                )),
                text: " world".into(),
                range_length: None,
            }]
        );

        // delete

        let transaction = transaction.invert(&old_doc);
        let old_doc = doc.text().clone();
        crate::editing::apply(&mut doc, &transaction, view);
        let changes = Client::changeset_to_changes(
            &old_doc,
            doc.text(),
            transaction.changes(),
            OffsetEncoding::Utf8,
        );

        // line: 0-based.
        // col: 0-based, gaps between chars.
        // 0 1 2 3 4 5 6 7 8 9 0 1
        // |h|e|l|l|o| |w|o|r|l|d|
        //           -------------
        // (0, 5)-(0, 11)
        assert_eq!(
            changes,
            &[lsp::TextDocumentContentChangeEvent {
                range: Some(lsp::Range::new(
                    lsp::Position::new(0, 5),
                    lsp::Position::new(0, 11)
                )),
                text: "".into(),
                range_length: None,
            }]
        );

        // replace

        // also tests that changes are layered, positions depend on previous changes.

        doc.set_selection(view, Selection::single(0, 5));
        let transaction = Transaction::change(
            doc.text(),
            vec![(0, 2, Some("aei".into())), (3, 5, Some("ou".into()))].into_iter(),
        );
        // aeilou
        let old_doc = doc.text().clone();
        crate::editing::apply(&mut doc, &transaction, view);
        let changes = Client::changeset_to_changes(
            &old_doc,
            doc.text(),
            transaction.changes(),
            OffsetEncoding::Utf8,
        );

        assert_eq!(
            changes,
            &[
                // 0 1 2 3 4 5
                // |h|e|l|l|o|
                // ----
                //
                // aeillo
                lsp::TextDocumentContentChangeEvent {
                    range: Some(lsp::Range::new(
                        lsp::Position::new(0, 0),
                        lsp::Position::new(0, 2)
                    )),
                    text: "aei".into(),
                    range_length: None,
                },
                // 0 1 2 3 4 5 6
                // |a|e|i|l|l|o|
                //         -----
                //
                // aeilou
                lsp::TextDocumentContentChangeEvent {
                    range: Some(lsp::Range::new(
                        lsp::Position::new(0, 4),
                        lsp::Position::new(0, 6)
                    )),
                    text: "ou".into(),
                    range_length: None,
                }
            ]
        );
    }

    #[test]
    fn test_line_ending() {
        assert_eq!(
            Document::default(
                Arc::new(ArcSwap::new(Arc::new(Config::default()))),
                Arc::new(ArcSwap::from_pointee(syntax::Loader::default()))
            )
            .text()
            .to_string(),
            editor_core::NATIVE_LINE_ENDING.as_str()
        );
    }
}
