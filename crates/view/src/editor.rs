use crate::{
    document::{
        DocumentOpenError, DocumentSavedEventFuture, DocumentSavedEventResult, Mode, SavePoint,
    },
    events::{DocumentDidClose, DocumentDidOpen, DocumentFocusLost},
    graphics::{CursorKind, Rect},
    handlers::Handlers,
    info::Info,
    input::KeyEvent,
    quicklist::{Quicklist, QuicklistEntry, QuicklistMatch, QuicklistPosition, QuicklistTarget},
    register::Registers,
    theme::{self, Theme},
    tree::{self, Tree},
    Document, DocumentId, View, ViewId,
};
use event::dispatch;
use loader::workspace_trust::{TrustQuery, WorkspaceTrust};
use vcs::DiffProviderRegistry;

use futures_util::stream::select_all::SelectAll;
use futures_util::StreamExt;
use lsp_client::{Call, LanguageServerId};
use tokio_stream::wrappers::UnboundedReceiverStream;

use std::{
    borrow::Cow,
    cell::Cell,
    collections::{BTreeMap, HashMap, VecDeque},
    fs,
    io::{self, stdin},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use tokio::{
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    time::{sleep, Duration, Instant, Sleep},
};

use anyhow::{anyhow, bail, Error};

use dap::{self as dap, registry::DebugAdapterId};
pub use editor_core::diagnostic::Severity;
use editor_core::file_watcher::{self, Watcher};
use editor_core::{
    auto_pairs::AutoPairs,
    diagnostic::DiagnosticProvider,
    movement::Direction,
    syntax::{self, config::LanguageServerFeature},
    Change, Position, Range, Selection, SpellingLanguage, Uri,
};
use lsp_client::{lsp, util::lsp_range_to_range};
use stdx::path::canonicalize;

use arc_swap::{
    access::{DynAccess, DynGuard},
    ArcSwap,
};

// Preserve existing configuration import paths; new code should use `crate::config`.
#[cfg(any(windows, not(target_arch = "wasm32")))]
pub use crate::config::get_terminal_provider;
pub use crate::config::{
    AutoReloadConfig, AutoReloadPoll, AutoSave, AutoSaveAfterDelay, BreadcrumbConfig,
    BreadcrumbPathOptions, BufferLine, BufferPickerConfig, Config, CursorShapeConfig,
    FileExplorerConfig, FilePickerConfig, GutterConfig, GutterLineNumbersConfig, GutterType,
    ImplicitTrustLevelConfig, IndentGuidesConfig, LineEndingConfig, LineNumber, LspConfig,
    ModeConfig, PickerStartPosition, SearchConfig, SmartTabConfig, StatusLineConfig,
    StatusLineElement, TerminalConfig, WhitespaceCharacters, WhitespaceConfig, WhitespaceRender,
    WhitespaceRenderValue, WordCompletion, WorkspaceTrustConfig, DEFAULT_AUTO_SAVE_DELAY,
};

pub use ui_core::terminal::KittyKeyboardProtocolConfig;

pub const DIR_STACK_CAP: usize = 10;

#[derive(Debug, Clone, Default)]
pub struct Breakpoint {
    pub id: Option<usize>,
    pub verified: bool,
    pub message: Option<String>,

    pub line: usize,
    pub column: Option<usize>,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

use futures_util::stream::{Flatten, Once};
type Diagnostics = BTreeMap<Uri, Vec<(lsp::Diagnostic, DiagnosticProvider)>>;

pub struct Editor {
    /// Current editing mode.
    pub mode: Mode,
    pub tree: Tree,
    pub next_document_id: DocumentId,
    pub documents: BTreeMap<DocumentId, Document>,

    // We Flatten<> to resolve the inner DocumentSavedEventFuture. For that we need a stream of streams, hence the Once<>.
    // https://stackoverflow.com/a/66875668
    pub saves: HashMap<DocumentId, UnboundedSender<Once<DocumentSavedEventFuture>>>,
    pub save_queue: SelectAll<Flatten<UnboundedReceiverStream<Once<DocumentSavedEventFuture>>>>,
    pub write_count: usize,

    pub count: Option<std::num::NonZeroUsize>,
    pub selected_register: Option<char>,
    pub registers: Registers,
    pub macro_recording: Option<(char, Vec<KeyEvent>)>,
    pub macro_replaying: Vec<char>,
    pub language_servers: lsp_client::Registry,
    pub diagnostics: Diagnostics,
    pub diff_providers: DiffProviderRegistry,

    pub debug_adapters: dap::registry::Registry,
    pub breakpoints: HashMap<PathBuf, Vec<Breakpoint>>,

    pub syn_loader: Arc<ArcSwap<syntax::Loader>>,
    pub theme_loader: Arc<theme::Loader>,
    /// last_theme is used for theme previews. We store the current theme here,
    /// and if previewing is cancelled, we can return to it.
    pub last_theme: Option<Theme>,
    /// The currently applied editor theme. While previewing a theme, the previewed theme
    /// is set here.
    pub theme: Theme,

    /// The primary Selection prior to starting a goto_line_number preview. This is
    /// restored when the preview is aborted, or added to the jumplist when it is
    /// confirmed.
    pub last_selection: Option<Selection>,
    pub quicklist: Quicklist,

    pub status_msg: Option<(Cow<'static, str>, Severity)>,
    pub autoinfo: Option<Info>,

    pub config: Arc<dyn DynAccess<Config>>,
    pub auto_pairs: Option<AutoPairs>,

    pub idle_timer: Pin<Box<Sleep>>,
    redraw_timer: Pin<Box<Sleep>>,
    last_motion: Option<Motion>,
    pub last_completion: Option<CompleteAction>,
    pub last_cwd: Option<PathBuf>,
    pub dir_stack: VecDeque<PathBuf>,

    pub exit_code: i32,

    pub config_events: (UnboundedSender<ConfigEvent>, UnboundedReceiver<ConfigEvent>),
    pub needs_redraw: bool,
    /// Cached position of the cursor calculated during rendering.
    /// The content of `cursor_cache` is returned by `Editor::cursor` if
    /// set to `Some(_)`. The value will be cleared after it's used.
    /// If `cursor_cache` is `None` then the `Editor::cursor` function will
    /// calculate the cursor position.
    ///
    /// `Some(None)` represents a cursor position outside of the visible area.
    /// This will just cause `Editor::cursor` to return `None`.
    ///
    /// This cache is only a performance optimization to
    /// avoid calculating the cursor position multiple
    /// times during rendering and should not be set by other functions.
    pub handlers: Handlers,

    pub mouse_down_range: Option<Range>,
    pub cursor_cache: CursorCache,
    /// Loaded spelling dictionaries keyed by language.
    pub dictionaries: HashMap<SpellingLanguage, Arc<crate::Dictionary>>,
    pub file_watcher: Watcher,
    pub workspace_trust: WorkspaceTrust,
}

pub type Motion = Box<dyn Fn(&mut Editor)>;

#[derive(Debug)]
pub enum EditorEvent {
    DocumentSaved(DocumentSavedEventResult),
    ConfigEvent(ConfigEvent),
    LanguageServerMessage((LanguageServerId, Call)),
    DebuggerEvent((DebugAdapterId, dap::Payload)),
    IdleTimer,
    Redraw,
}

#[derive(Debug, Clone)]
pub enum ConfigEvent {
    Refresh,
    Update(Box<Config>),
    ThemeChanged,
}

enum ThemeAction {
    Set,
    Preview,
}

#[derive(Debug, Clone)]
pub enum CompleteAction {
    Triggered,
    /// A savepoint of the currently selected completion. The savepoint
    /// MUST be restored before sending any event to the LSP
    Selected {
        savepoint: Arc<SavePoint>,
    },
    Applied {
        trigger_offset: usize,
        changes: Vec<Change>,
        placeholder: bool,
    },
}

#[derive(Debug, Copy, Clone)]
pub enum Action {
    Load,
    Replace,
    HorizontalSplit,
    VerticalSplit,
}

impl Action {
    /// Whether to align the view to the cursor after executing this action
    pub fn align_view(&self, view: &View, new_doc: DocumentId) -> bool {
        !matches!((self, view.doc == new_doc), (Action::Load, false))
    }
}

/// Error thrown on failed document closed
pub enum CloseError {
    /// Document doesn't exist
    DoesNotExist,
    /// Buffer is modified
    BufferModified(String),
    /// Document failed to save
    SaveError(anyhow::Error),
}

impl Editor {
    pub fn new(
        mut area: Rect,
        theme_loader: Arc<theme::Loader>,
        syn_loader: Arc<ArcSwap<syntax::Loader>>,
        config: Arc<dyn DynAccess<Config>>,
        handlers: Handlers,
        workspace_trust: WorkspaceTrust,
    ) -> Self {
        let language_servers = lsp_client::Registry::new(syn_loader.clone());
        let conf = config.load();
        let auto_pairs = (&conf.auto_pairs).into();

        // Initialize file watcher and diff providers
        let file_watcher = Watcher::new(&conf.file_watcher);
        let diff_providers = DiffProviderRegistry::default();

        // HAXX: offset the render area height by 1 to account for prompt/commandline
        area.height -= 1;

        Self {
            mode: Mode::Normal,
            tree: Tree::new(area),
            next_document_id: DocumentId::default(),
            documents: BTreeMap::new(),
            saves: HashMap::new(),
            save_queue: SelectAll::new(),
            write_count: 0,
            count: None,
            selected_register: None,
            macro_recording: None,
            macro_replaying: Vec::new(),
            theme: theme_loader.default(),
            language_servers,
            diagnostics: Diagnostics::new(),
            diff_providers,
            debug_adapters: dap::registry::Registry::new(),
            breakpoints: HashMap::new(),
            syn_loader,
            theme_loader,
            last_theme: None,
            last_selection: None,
            quicklist: Quicklist::default(),
            registers: Registers::new(Box::new(arc_swap::access::Map::new(
                Arc::clone(&config),
                |config: &Config| &config.clipboard_provider,
            ))),
            status_msg: None,
            autoinfo: None,
            idle_timer: Box::pin(sleep(conf.idle_timeout)),
            redraw_timer: Box::pin(sleep(Duration::MAX)),
            last_motion: None,
            last_completion: None,
            last_cwd: None,
            config,
            auto_pairs,
            exit_code: 0,
            config_events: unbounded_channel(),
            needs_redraw: false,
            handlers,
            mouse_down_range: None,
            cursor_cache: CursorCache::default(),
            dictionaries: HashMap::new(),
            file_watcher,
            dir_stack: VecDeque::with_capacity(DIR_STACK_CAP),
            workspace_trust,
        }
    }

    pub fn apply_motion<F: Fn(&mut Self) + 'static>(&mut self, motion: F) {
        motion(self);
        self.last_motion = Some(Box::new(motion));
    }

    pub fn repeat_last_motion(&mut self, count: usize) {
        if let Some(motion) = self.last_motion.take() {
            for _ in 0..count {
                motion(self);
            }
            self.last_motion = Some(motion);
        }
    }
    /// Current editing mode for the [`Editor`].
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn config(&self) -> DynGuard<Config> {
        self.config.load()
    }

    /// Call if the config has changed to let the editor update all
    /// relevant members.
    pub fn refresh_config(&mut self, old_config: &Config) {
        let config = self.config();
        self.auto_pairs = (&config.auto_pairs).into();
        self.reset_idle_timer();
        self._refresh();
        event::dispatch(crate::events::ConfigDidChange {
            editor: self,
            old: old_config,
            new: &config,
        })
    }

    pub fn clear_idle_timer(&mut self) {
        // equivalent to internal Instant::far_future() (30 years)
        self.idle_timer
            .as_mut()
            .reset(Instant::now() + Duration::from_secs(86400 * 365 * 30));
    }

    pub fn reset_idle_timer(&mut self) {
        let config = self.config();
        self.idle_timer
            .as_mut()
            .reset(Instant::now() + config.idle_timeout);
    }

    pub fn clear_status(&mut self) {
        self.status_msg = None;
    }

    #[inline]
    pub fn set_status<T: Into<Cow<'static, str>>>(&mut self, status: T) {
        let status = status.into();
        log::debug!("editor status: {}", status);
        self.status_msg = Some((status, Severity::Info));
    }

    #[cold]
    #[inline(never)]
    pub fn set_error<C, M>(&mut self, message: M)
    where
        C: Into<Cow<'static, str>>,
        M: FnOnce() -> C,
    {
        let error = message().into();
        log::debug!("editor error: {}", error);
        self.status_msg = Some((error, Severity::Error));
    }

    #[cold]
    #[inline(never)]
    pub fn set_warning<C, M>(&mut self, message: M)
    where
        C: Into<Cow<'static, str>>,
        M: FnOnce() -> C,
    {
        let warning = message().into();
        log::warn!("editor warning: {}", warning);
        self.status_msg = Some((warning, Severity::Warning));
    }

    #[inline]
    pub fn get_status(&self) -> Option<(&Cow<'static, str>, &Severity)> {
        self.status_msg.as_ref().map(|(status, sev)| (status, sev))
    }

    /// Returns true if the current status is an error
    #[inline]
    pub fn is_err(&self) -> bool {
        self.status_msg
            .as_ref()
            .map(|(_, sev)| *sev == Severity::Error)
            .unwrap_or(false)
    }

    pub fn unset_theme_preview(&mut self) -> anyhow::Result<()> {
        if let Some(last_theme) = self.last_theme.take() {
            self.set_theme(last_theme)?;
        }
        // None likely occurs when the user types ":theme" and then exits before previewing
        Ok(())
    }

    pub fn set_theme_preview(&mut self, theme: Theme) -> anyhow::Result<()> {
        self.set_theme_impl(theme, ThemeAction::Preview)
    }

    pub fn set_theme(&mut self, theme: Theme) -> anyhow::Result<()> {
        self.set_theme_impl(theme, ThemeAction::Set)
    }

    fn set_theme_impl(&mut self, theme: Theme, preview: ThemeAction) -> anyhow::Result<()> {
        // `ui.selection` is the only scope required to be able to render a theme.
        if theme.find_highlight_exact("ui.selection").is_none() {
            bail!("Invalid theme: `ui.selection` required");
        }

        let scopes = theme.scopes();
        (*self.syn_loader).load().set_scopes(scopes.to_vec());

        match preview {
            ThemeAction::Preview => {
                let last_theme = std::mem::replace(&mut self.theme, theme);
                // only insert on first preview: this will be the last theme the user has saved
                self.last_theme.get_or_insert(last_theme);
            }
            ThemeAction::Set => {
                self.last_theme = None;
                self.theme = theme;
            }
        }

        self._refresh();
        self.config_events.0.send(ConfigEvent::ThemeChanged)?;

        Ok(())
    }

    #[inline]
    pub fn language_server_by_id(
        &self,
        language_server_id: LanguageServerId,
    ) -> Option<&lsp_client::Client> {
        self.language_servers
            .get_by_id(language_server_id)
            .map(|client| &**client)
    }

    /// Refreshes the language server for a given document
    pub fn refresh_language_servers(&mut self, doc_id: DocumentId) {
        self.launch_language_servers(doc_id)
    }

    /// moves/renames a path, invoking any event handlers (currently only lsp)
    /// and calling `set_doc_path` if the file is open in the editor
    pub fn move_path(&mut self, old_path: &Path, new_path: &Path) -> io::Result<()> {
        let new_path = canonicalize(new_path);
        // sanity check
        if old_path == new_path {
            return Ok(());
        }
        let is_dir = old_path.is_dir();
        let language_servers: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| client.is_initialized())
            .cloned()
            .collect();
        for language_server in language_servers {
            let Some(request) = language_server.will_rename(old_path, &new_path, is_dir) else {
                continue;
            };
            let edit = match lsp_client::block_on(request) {
                Ok(edit) => edit.unwrap_or_default(),
                Err(err) => {
                    log::error!("invalid willRename response: {err:?}");
                    continue;
                }
            };
            if let Err(err) = self.apply_workspace_edit(language_server.offset_encoding(), &edit) {
                log::error!("failed to apply workspace edit: {err:?}")
            }
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
                .file_changed(old_path.to_owned(), file_watcher::EventType::Delete);
        }
        if !new_watched {
            self.language_servers
                .file_event_handler
                .file_changed(new_path, file_watcher::EventType::Create);
        }
        Ok(())
    }

    pub fn create_path(&mut self, path: &Path, is_dir: bool) -> io::Result<()> {
        let path = canonicalize(path);
        let language_servers: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| client.is_initialized())
            .cloned()
            .collect();
        for language_server in language_servers {
            let Some(request) = language_server.will_create(&path, is_dir) else {
                continue;
            };
            let edit = match lsp_client::block_on(request) {
                Ok(edit) => edit.unwrap_or_default(),
                Err(err) => {
                    log::error!("invalid willCreate response: {err:?}");
                    continue;
                }
            };
            if let Err(err) = self.apply_workspace_edit(language_server.offset_encoding(), &edit) {
                log::error!("failed to apply workspace edit: {err:?}")
            }
        }

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
                .file_changed(path, file_watcher::EventType::Create);
        }
        Ok(())
    }

    pub fn delete_path(&mut self, path: &Path, recursive: bool) -> io::Result<()> {
        let path = canonicalize(path);
        let watched = self.file_watcher.is_watching(&path);
        let is_dir = path.is_dir();
        let language_servers: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| client.is_initialized())
            .cloned()
            .collect();
        for language_server in language_servers {
            let Some(request) = language_server.will_delete(&path, is_dir) else {
                continue;
            };
            let edit = match lsp_client::block_on(request) {
                Ok(edit) => edit.unwrap_or_default(),
                Err(err) => {
                    log::error!("invalid willDelete response: {err:?}");
                    continue;
                }
            };
            if let Err(err) = self.apply_workspace_edit(language_server.offset_encoding(), &edit) {
                log::error!("failed to apply workspace edit: {err:?}")
            }
        }

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
                .file_changed(path, file_watcher::EventType::Delete);
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
            // if we are open in LSPs send did_close notification
            for language_server in doc.language_servers() {
                language_server.text_document_did_close(doc.identifier());
            }
        }
        // we need to clear the list of language servers here so that
        // refresh_doc_language/refresh_language_servers doesn't resend
        // text_document_did_close. Since we called `text_document_did_close`
        // we have fully unregistered this document from its LS
        doc.language_servers.clear();
        doc.set_path(Some(path));
        doc.detect_editor_config();
        self.refresh_doc_language(doc_id);
        self.refresh_vcs_watches();
    }

    pub fn refresh_doc_language(&mut self, doc_id: DocumentId) {
        let loader = self.syn_loader.load();
        let doc = doc_mut!(self, &doc_id);
        doc.detect_language(&loader);
        doc.detect_editor_config();
        doc.detect_indent_and_line_ending();
        self.refresh_language_servers(doc_id);
        let doc = doc_mut!(self, &doc_id);
        let diagnostics = Editor::doc_diagnostics(&self.language_servers, &self.diagnostics, doc);
        doc.replace_diagnostics(diagnostics, &[], None);
        doc.reset_all_inlay_hints();
        doc.clear_document_symbols();
        self.refresh_spelling(doc_id);
    }

    /// Launch a language server for a given document
    pub fn launch_language_servers(&mut self, doc_id: DocumentId) {
        if !self.config().lsp.enable {
            return;
        }
        // if doc doesn't have a URL it's a scratch buffer, ignore it
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        let Some(doc_url) = doc.url() else {
            return;
        };
        let (lang, path) = (doc.language.clone(), doc.path());
        let config = doc.config.load();
        let root_dirs = &config.workspace_lsp_roots;

        let workspace = doc.workspace_root();
        let trust = self.workspace_trust.query(workspace, TrustQuery::Lsp);
        if !trust.is_trusted() {
            return;
        }

        // store only successfully started language servers
        let language_servers = lang.as_ref().map_or_else(HashMap::default, |language| {
            self.language_servers
                .get(language, path, root_dirs, config.lsp.snippets)
                .filter_map(|(lang, client)| match client {
                    Ok(client) => Some((lang, client)),
                    Err(err) => {
                        if let lsp_client::Error::ExecutableNotFound(err) = err {
                            // Silence by default since some language servers might just not be installed
                            log::debug!(
                                "Language server not found for `{}` {} {}", language.scope, lang, err,
                            );
                        } else {
                            log::error!(
                                "Failed to initialize the language servers for `{}` - `{}` {{ {} }}",
                                language.scope,
                                lang,
                                err
                            );
                        }
                        None
                    }
                })
                .collect::<HashMap<_, _>>()
        });

        if language_servers.is_empty() && doc.language_servers.is_empty() {
            return;
        }

        let language_id = doc.language_id().map(ToOwned::to_owned).unwrap_or_default();

        // only spawn new language servers if the servers aren't the same
        let doc_language_servers_not_in_registry =
            doc.language_servers.iter().filter(|(name, doc_ls)| {
                language_servers
                    .get(*name)
                    .is_none_or(|ls| ls.id() != doc_ls.id())
            });

        for (_, language_server) in doc_language_servers_not_in_registry {
            language_server.text_document_did_close(doc.identifier());
        }

        let language_servers_not_in_doc = language_servers.iter().filter(|(name, ls)| {
            doc.language_servers
                .get(*name)
                .is_none_or(|doc_ls| ls.id() != doc_ls.id())
        });

        for (_, language_server) in language_servers_not_in_doc {
            // TODO: this now races with on_init code if the init happens too quickly
            language_server.text_document_did_open(
                doc_url.clone(),
                doc.version(),
                doc.text(),
                language_id.clone(),
            );
        }

        doc.language_servers = language_servers;
    }

    fn _refresh(&mut self) {
        let config = self.config();

        // Reset the inlay hints annotations *before* updating the views, that way we ensure they
        // will disappear during the `.sync_change(doc)` call below.
        //
        // We can't simply check this config when rendering because inlay hints are only parts of
        // the possible annotations, and others could still be active, so we need to selectively
        // drop the inlay hints.
        if !config.lsp.display_inlay_hints {
            for doc in self.documents_mut() {
                doc.reset_all_inlay_hints();
            }
        }

        for (view, _) in self.tree.views_mut() {
            let doc = doc_mut!(self, &view.doc);
            view.sync_changes(doc);
            view.gutters = config.gutters.clone();
            view.ensure_cursor_in_view(doc, config.scrolloff)
        }
    }

    fn replace_document_in_view(&mut self, current_view: ViewId, doc_id: DocumentId) {
        let scrolloff = self.config().scrolloff;
        let view = self.tree.get_mut(current_view);

        view.doc = doc_id;
        let doc = doc_mut!(self, &doc_id);

        doc.ensure_view_init(view.id);
        view.sync_changes(doc);
        doc.mark_as_focused();

        view.ensure_cursor_in_view(doc, scrolloff)
    }

    pub fn switch(&mut self, id: DocumentId, action: Action) {
        use crate::tree::Layout;

        if !self.documents.contains_key(&id) {
            log::error!("cannot switch to document that does not exist (anymore)");
            return;
        }

        if !matches!(action, Action::Load) {
            self.enter_normal_mode();
        }

        let focust_lost = match action {
            Action::Replace => {
                let (view, doc) = current_ref!(self);
                // If the current view is an empty scratch buffer and is not displayed in any other views, delete it.
                // Boolean value is determined before the call to `view_mut` because the operation requires a borrow
                // of `self.tree`, which is mutably borrowed when `view_mut` is called.
                let remove_empty_scratch = !doc.is_modified()
                    // If the buffer has no path and is not modified, it is an empty scratch buffer.
                    && doc.path().is_none()
                    // If the buffer we are changing to is not this buffer
                    && id != doc.id
                    // Ensure the buffer is not displayed in any other splits.
                    && !self
                        .tree
                        .traverse()
                        .any(|(_, v)| v.doc == doc.id && v.id != view.id);

                let (view, doc) = current!(self);
                let view_id = view.id;

                // Append any outstanding changes to history in the old document.
                doc.append_changes_to_history(view);

                if remove_empty_scratch {
                    // Copy `doc.id` into a variable before calling `self.documents.remove`, which requires a mutable
                    // borrow, invalidating direct access to `doc.id`.
                    let id = doc.id;
                    self.documents.remove(&id);

                    // Remove the scratch buffer from any jumplists
                    for (view, _) in self.tree.views_mut() {
                        view.remove_document(&id);
                    }
                } else {
                    let jump = (view.doc, doc.selection(view.id).clone());
                    view.push_jump(doc, jump);
                    // Set last accessed doc if it is a different document
                    if doc.id != id {
                        view.add_to_history(view.doc);
                        // Set last modified doc if modified and last modified doc is different
                        if std::mem::take(&mut doc.modified_since_accessed)
                            && view.last_modified_docs[0] != Some(view.doc)
                        {
                            view.last_modified_docs = [Some(view.doc), view.last_modified_docs[0]];
                        }
                    }
                }

                self.replace_document_in_view(view_id, id);

                dispatch(DocumentFocusLost {
                    editor: self,
                    doc: id,
                });
                return;
            }
            Action::Load => {
                let view_id = view!(self).id;
                let doc = doc_mut!(self, &id);
                doc.ensure_view_init(view_id);
                doc.mark_as_focused();
                return;
            }
            Action::HorizontalSplit | Action::VerticalSplit => {
                let focus_lost = self.tree.try_get(self.tree.focus).map(|view| view.doc);
                // copy the current view, unless there is no view yet
                let view = self
                    .tree
                    .try_get(self.tree.focus)
                    .filter(|v| id == v.doc) // Different Document
                    .cloned()
                    .unwrap_or_else(|| View::new(id, self.config().gutters.clone()));
                let view_id = self.tree.split(
                    view,
                    match action {
                        Action::HorizontalSplit => Layout::Horizontal,
                        Action::VerticalSplit => Layout::Vertical,
                        _ => unreachable!(),
                    },
                );
                // initialize selection for view
                let doc = doc_mut!(self, &id);
                doc.ensure_view_init(view_id);
                doc.mark_as_focused();
                focus_lost
            }
        };

        self._refresh();
        if let Some(focus_lost) = focust_lost {
            dispatch(DocumentFocusLost {
                editor: self,
                doc: focus_lost,
            });
        }
    }

    /// Generate an id for a new document and register it.
    fn new_document(&mut self, mut doc: Document) -> DocumentId {
        let id = self.next_document_id;
        // Safety: adding 1 from 1 is fine, practically impossible to reach usize max
        self.next_document_id =
            DocumentId(unsafe { NonZeroUsize::new_unchecked(self.next_document_id.0.get() + 1) });
        doc.id = id;
        doc.syntax_handler = Some(self.handlers.syntax.clone());
        doc.initialize_syntax(self.syn_loader.load_full());
        doc.detect_spelling_languages();
        self.documents.insert(id, doc);
        self.refresh_vcs_watches();

        let (save_sender, save_receiver) = tokio::sync::mpsc::unbounded_channel();
        self.saves.insert(id, save_sender);

        let stream = UnboundedReceiverStream::new(save_receiver).flatten();
        self.save_queue.push(stream);

        id
    }

    fn new_file_from_document(&mut self, action: Action, doc: Document) -> DocumentId {
        let id = self.new_document(doc);
        self.switch(id, action);
        self.refresh_spelling(id);
        id
    }

    pub fn new_file(&mut self, action: Action) -> DocumentId {
        self.new_file_from_document(
            action,
            Document::default(self.config.clone(), self.syn_loader.clone()),
        )
    }

    /// Create the initial scratch document shown when Mitos starts without a file.
    pub fn new_file_welcome(&mut self) -> DocumentId {
        self.new_file_from_document(
            Action::VerticalSplit,
            Document::default(self.config.clone(), self.syn_loader.clone()).with_welcome(),
        )
    }

    pub fn new_file_from_stdin(&mut self, action: Action) -> Result<DocumentId, Error> {
        let (stdin, encoding, has_bom) = crate::document::read_to_string(&mut stdin(), None)?;
        let doc = Document::from(
            editor_core::Rope::default(),
            Some((encoding, has_bom)),
            self.config.clone(),
            self.syn_loader.clone(),
        );
        let doc_id = self.new_file_from_document(action, doc);
        let doc = doc_mut!(self, &doc_id);
        let view = view_mut!(self);
        doc.ensure_view_init(view.id);
        let transaction =
            editor_core::Transaction::insert(doc.text(), doc.selection(view.id), stdin.into())
                .with_selection(Selection::point(0));
        doc.apply(&transaction, view.id);
        doc.append_changes_to_history(view);
        Ok(doc_id)
    }

    pub fn document_id_by_path(&self, path: &Path) -> Option<DocumentId> {
        self.document_by_path(path).map(|doc| doc.id)
    }

    // ??? possible use for integration tests
    pub fn open(&mut self, path: &Path, action: Action) -> Result<DocumentId, DocumentOpenError> {
        let path = stdx::path::canonicalize(path);
        let id = self.document_id_by_path(&path);

        let id = if let Some(id) = id {
            id
        } else {
            let mut doc = Document::open(
                &path,
                None,
                true,
                self.config.clone(),
                self.syn_loader.clone(),
            )?;

            let diagnostics =
                Editor::doc_diagnostics(&self.language_servers, &self.diagnostics, &doc);
            doc.replace_diagnostics(diagnostics, &[], None);

            let trust_full = self
                .workspace_trust
                .query(doc.workspace_root(), TrustQuery::Git)
                .is_trusted();
            if !doc.is_binary()
                && let Some(diff_base) = self.diff_providers.get_diff_base(&path, trust_full)
            {
                doc.set_diff_base(diff_base);
            }
            doc.set_version_control_head(
                self.diff_providers.get_current_head_name(&path, trust_full),
            );

            let id = self.new_document(doc);
            self.launch_language_servers(id);

            event::dispatch(DocumentDidOpen {
                editor: self,
                doc: id,
            });

            id
        };

        self.switch(id, action);

        Ok(id)
    }

    pub fn close(&mut self, id: ViewId) {
        // Remove selections for the closed view on all documents.
        for doc in self.documents_mut() {
            doc.remove_view(id);
        }
        self.tree.remove(id);
        self._refresh();
    }

    pub fn close_document(&mut self, doc_id: DocumentId, force: bool) -> Result<(), CloseError> {
        let doc = match self.documents.get(&doc_id) {
            Some(doc) => doc,
            None => return Err(CloseError::DoesNotExist),
        };
        if !force && doc.is_modified() {
            return Err(CloseError::BufferModified(doc.display_name().into_owned()));
        }

        // This will also disallow any follow-up writes
        self.saves.remove(&doc_id);

        enum Action {
            Close(ViewId),
            ReplaceDoc(ViewId, DocumentId),
        }

        let actions: Vec<Action> = self
            .tree
            .views_mut()
            .filter_map(|(view, _focus)| {
                view.remove_document(&doc_id);

                if view.doc == doc_id {
                    // something was previously open in the view, switch to previous doc
                    if let Some(prev_doc) = view.docs_access_history.pop() {
                        Some(Action::ReplaceDoc(view.id, prev_doc))
                    } else {
                        // only the document that is being closed was in the view, close it
                        Some(Action::Close(view.id))
                    }
                } else {
                    None
                }
            })
            .collect();

        for action in actions {
            match action {
                Action::Close(view_id) => {
                    self.close(view_id);
                }
                Action::ReplaceDoc(view_id, doc_id) => {
                    self.replace_document_in_view(view_id, doc_id);
                }
            }
        }

        let doc = self.documents.remove(&doc_id).unwrap();
        self.refresh_vcs_watches();

        // If the document we removed was visible in all views, we will have no more views. We don't
        // want to close the editor just for a simple buffer close, so we need to create a new view
        // containing either an existing document, or a brand new document.
        if self.tree.views().next().is_none() {
            let doc_id = self
                .documents
                .iter()
                .map(|(&doc_id, _)| doc_id)
                .next()
                .unwrap_or_else(|| {
                    self.new_document(Document::default(
                        self.config.clone(),
                        self.syn_loader.clone(),
                    ))
                });
            let view = View::new(doc_id, self.config().gutters.clone());
            let view_id = self.tree.insert(view);
            let doc = doc_mut!(self, &doc_id);
            doc.ensure_view_init(view_id);
            doc.mark_as_focused();
        }

        self._refresh();

        event::dispatch(DocumentDidClose { editor: self, doc });

        Ok(())
    }

    pub fn save<P: Into<PathBuf>>(
        &mut self,
        doc_id: DocumentId,
        path: Option<P>,
        force: bool,
    ) -> anyhow::Result<()> {
        // convert a channel of futures to pipe into main queue one by one
        // via stream.then() ? then push into main future

        let path = path.map(|path| path.into());
        let doc = doc_mut!(self, &doc_id);
        // the path that will be written: the override, else the document's own path
        let save_path = path.clone().or_else(|| doc.path().map(ToOwned::to_owned));
        let created = save_path.as_ref().is_some_and(|path| !path.exists());
        let doc_save_future = doc.save(path, force)?;

        // When a file is written to, notify the file event handler, unless the
        // watcher already covers it, in which case filesentry reports the write.
        let handler = self.language_servers.file_event_handler.clone();
        let watched = save_path
            .as_deref()
            .is_some_and(|path| self.file_watcher.is_watching(path));
        let future = async move {
            let res = doc_save_future.await;
            if !watched && let Ok(event) = &res {
                handler.file_changed(
                    event.path.clone(),
                    if created {
                        file_watcher::EventType::Create
                    } else {
                        file_watcher::EventType::Modified
                    },
                );
            }
            res
        };

        use futures_util::stream;

        self.saves
            .get(&doc_id)
            .ok_or_else(|| anyhow::format_err!("saves are closed for this document!"))?
            .send(stream::once(Box::pin(future)))
            .map_err(|err| anyhow!("failed to send save event: {}", err))?;

        self.write_count += 1;

        Ok(())
    }

    pub fn resize(&mut self, area: Rect) {
        if self.tree.resize(area) {
            self._refresh();
        };
    }

    pub fn focus(&mut self, view_id: ViewId) {
        if self.tree.focus == view_id {
            return;
        }

        // Reset mode to normal and ensure any pending changes are committed in the old document.
        self.enter_normal_mode();
        let (view, doc) = current!(self);
        doc.append_changes_to_history(view);
        self.ensure_cursor_in_view(view_id);
        // Update jumplist selections with new document changes.
        for (view, _focused) in self.tree.views_mut() {
            let doc = doc_mut!(self, &view.doc);
            view.sync_changes(doc);
        }

        let prev_id = std::mem::replace(&mut self.tree.focus, view_id);
        doc_mut!(self).mark_as_focused();

        let focus_lost = self.tree.get(prev_id).doc;
        dispatch(DocumentFocusLost {
            editor: self,
            doc: focus_lost,
        });
    }

    pub fn focus_next(&mut self) {
        self.focus(self.tree.next());
    }

    pub fn focus_prev(&mut self) {
        self.focus(self.tree.prev());
    }

    pub fn focus_direction(&mut self, direction: tree::Direction) {
        let current_view = self.tree.focus;
        if let Some(id) = self.tree.find_split_in_direction(current_view, direction) {
            self.focus(id)
        }
    }

    pub fn swap_split_in_direction(&mut self, direction: tree::Direction) {
        self.tree.swap_split_in_direction(direction);
    }

    pub fn transpose_view(&mut self) {
        self.tree.transpose();
    }

    pub fn should_close(&self) -> bool {
        self.tree.is_empty()
    }

    pub fn ensure_cursor_in_view(&mut self, id: ViewId) {
        let config = self.config();
        let view = self.tree.get(id);
        let doc = doc_mut!(self, &view.doc);
        view.ensure_cursor_in_view(doc, config.scrolloff)
    }

    #[inline]
    pub fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.get(&id)
    }

    #[inline]
    pub fn document_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.documents.get_mut(&id)
    }

    /// Finish pending syntax initialization for an explicit syntax-dependent command.
    /// Normal file loading publishes syntax through the editor's completion sender.
    pub fn ensure_syntax(&mut self, id: DocumentId) {
        if self
            .document_mut(id)
            .is_some_and(Document::finish_syntax_initialization)
        {
            self.refresh_spelling(id);
        }
    }

    #[inline]
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    #[inline]
    pub fn documents_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        self.documents.values_mut()
    }

    pub fn document_by_path<P: AsRef<Path>>(&self, path: P) -> Option<&Document> {
        self.documents()
            .find(|doc| doc.path().is_some_and(|p| p == path.as_ref()))
    }

    pub fn document_by_path_mut<P: AsRef<Path>>(&mut self, path: P) -> Option<&mut Document> {
        self.documents_mut()
            .find(|doc| doc.path().is_some_and(|p| p == path.as_ref()))
    }

    /// Returns all supported diagnostics for the document
    pub fn doc_diagnostics<'a>(
        language_servers: &'a lsp_client::Registry,
        diagnostics: &'a Diagnostics,
        document: &Document,
    ) -> impl Iterator<Item = editor_core::Diagnostic> + 'a + use<'a> {
        Editor::doc_diagnostics_with_filter(language_servers, diagnostics, document, |_, _| true)
    }

    /// Returns all supported diagnostics for the document
    /// filtered by `filter` which is invocated with the raw `lsp::Diagnostic` and the language server id it came from
    pub fn doc_diagnostics_with_filter<'a, F>(
        language_servers: &'a lsp_client::Registry,
        diagnostics: &'a Diagnostics,
        document: &Document,
        filter: F,
    ) -> impl Iterator<Item = editor_core::Diagnostic> + 'a + use<'a, F>
    where
        F: Fn(&lsp::Diagnostic, &DiagnosticProvider) -> bool + 'a,
    {
        let text = document.text().clone();
        let language_config = document.language.clone();
        document
            .uri()
            .and_then(|uri| diagnostics.get(&uri))
            .map(|diags| {
                diags.iter().filter_map(move |(diagnostic, provider)| {
                    let server_id = provider.language_server_id()?;
                    let ls = language_servers.get_by_id(server_id)?;
                    language_config
                        .as_ref()
                        .and_then(|c| {
                            c.language_servers.iter().find(|features| {
                                features.name == ls.name()
                                    && features.has_feature(LanguageServerFeature::Diagnostics)
                            })
                        })
                        .and_then(|_| {
                            if filter(diagnostic, provider) {
                                Document::lsp_diagnostic_to_diagnostic(
                                    &text,
                                    language_config.as_deref(),
                                    diagnostic,
                                    provider.clone(),
                                    ls.offset_encoding(),
                                )
                            } else {
                                None
                            }
                        })
                })
            })
            .into_iter()
            .flatten()
    }

    /// Gets the primary cursor position in screen coordinates,
    /// or `None` if the primary cursor is not visible on screen.
    pub fn cursor(&self) -> (Option<Position>, CursorKind) {
        let config = self.config();
        let (view, doc) = current_ref!(self);
        if let Some(mut pos) = self.cursor_cache.get(view, doc) {
            let inner = view.inner_area(doc);
            pos.col += inner.x as usize;
            pos.row += inner.y as usize;
            let cursorkind = config.cursor_shape.from_mode(self.mode);
            (Some(pos), cursorkind)
        } else {
            (None, CursorKind::default())
        }
    }

    /// Closes language servers with timeout. The default timeout is 10000 ms, use
    /// `timeout` parameter to override this.
    pub async fn close_language_servers(&self, timeout: Option<u64>) {
        // Remove all language servers from the file event handler.
        // Note: this is non-blocking.
        for client in self.language_servers.iter_clients() {
            self.language_servers
                .file_event_handler
                .remove_client(client.id());
        }

        // Enqueue shutdown+exit for every server (non-blocking fire-and-forget).
        for client in self.language_servers.iter_clients() {
            client.force_shutdown();
        }

        // Wait until shutdown+exit have actually been written to each server's stdin
        // before the runtime (and the pipes) are torn down, so well-behaved servers
        // can act on `exit` before kill_on_drop reaps them. This waits only on our
        // own outbound write -- not on any server response -- so a slow server (e.g.
        // gopls flushing logs) doesn't delay it. Capped so a wedged write can't hang
        // the quit.
        let cap = Duration::from_millis(timeout.unwrap_or(1000));
        let _ = tokio::time::timeout(cap, async {
            for client in self.language_servers.iter_clients() {
                client.wait_shutdown_flushed().await;
            }
        })
        .await;
    }

    pub async fn wait_event(&mut self) -> EditorEvent {
        // the loop only runs once or twice and would be better implemented with a recursion + const generic
        // however due to limitations with async functions that can not be implemented right now
        loop {
            tokio::select! {
                biased;

                Some(event) = self.save_queue.next() => {
                    self.write_count -= 1;
                    return EditorEvent::DocumentSaved(event)
                }
                Some(config_event) = self.config_events.1.recv() => {
                    return EditorEvent::ConfigEvent(config_event)
                }
                Some(message) = self.language_servers.incoming.next() => {
                    return EditorEvent::LanguageServerMessage(message)
                }
                Some(event) = self.debug_adapters.incoming.next() => {
                    return EditorEvent::DebuggerEvent(event)
                }

                _ = event::redraw_requested() => {
                    if  !self.needs_redraw{
                        self.needs_redraw = true;
                        let timeout = Instant::now() + Duration::from_millis(33);
                        if timeout < self.idle_timer.deadline() && timeout < self.redraw_timer.deadline(){
                            self.redraw_timer.as_mut().reset(timeout)
                        }
                    }
                }

                _ = &mut self.redraw_timer  => {
                    self.redraw_timer.as_mut().reset(Instant::now() + Duration::from_secs(86400 * 365 * 30));
                    return EditorEvent::Redraw
                }
                _ = &mut self.idle_timer  => {
                    return EditorEvent::IdleTimer
                }
            }
        }
    }

    pub async fn flush_writes(&mut self) -> anyhow::Result<()> {
        while self.write_count > 0 {
            if let Some(save_event) = self.save_queue.next().await {
                self.write_count -= 1;

                let save_event = match save_event {
                    Ok(event) => event,
                    Err(err) => {
                        self.set_error(|| err.to_string());
                        bail!(err);
                    }
                };

                let doc = doc_mut!(self, &save_event.doc_id);
                doc.set_last_saved_revision(save_event.revision, save_event.save_time);
            }
        }

        Ok(())
    }

    /// Switches the editor into normal mode.
    pub fn enter_normal_mode(&mut self) {
        use editor_core::graphemes;

        if self.mode == Mode::Normal {
            return;
        }

        self.mode = Mode::Normal;
        let (view, doc) = current!(self);

        try_restore_indent(doc, view);

        // if leaving append mode, move cursor back by 1
        if doc.restore_cursor {
            let text = doc.text().slice(..);
            let selection = doc.selection(view.id).clone().transform(|range| {
                let mut head = range.to();
                if range.head > range.anchor {
                    head = graphemes::prev_grapheme_boundary(text, head);
                }

                Range::new(range.from(), head)
            });

            doc.set_selection(view.id, selection);
            doc.restore_cursor = false;
        }
    }

    pub fn current_stack_frame(&self) -> Option<&dap::StackFrame> {
        self.debug_adapters.current_stack_frame()
    }

    /// Returns the id of a view that this doc contains a selection for,
    /// making sure it is synced with the current changes
    /// if possible or there are no selections returns current_view
    /// otherwise uses an arbitrary view
    pub fn get_synced_view_id(&mut self, id: DocumentId) -> ViewId {
        let current_view = view_mut!(self);
        let doc = self.documents.get_mut(&id).unwrap();
        if doc.selections().contains_key(&current_view.id) {
            // only need to sync current view if this is not the current doc
            if current_view.doc != id {
                current_view.sync_changes(doc);
            }
            current_view.id
        } else if let Some(view_id) = doc.selections().keys().next() {
            let view_id = *view_id;
            let view = self.tree.get_mut(view_id);
            view.sync_changes(doc);
            view_id
        } else {
            doc.ensure_view_init(current_view.id);
            current_view.id
        }
    }

    /// Include repositories for every open document, including linked worktrees.
    pub fn refresh_vcs_watches(&mut self) {
        let mut paths = Vec::new();
        if self.config().file_watcher.watch_vcs {
            for doc in self.documents.values() {
                let Some(path) = doc.path().and_then(|path| path.parent()) else {
                    continue;
                };
                let trust_full = self
                    .workspace_trust
                    .query(
                        doc.workspace_root(),
                        loader::workspace_trust::TrustQuery::Git,
                    )
                    .is_trusted();
                paths.extend(self.diff_providers.get_watched_paths(path, trust_full));
            }
        }
        self.file_watcher.set_extra_watched_paths(paths);
    }

    pub fn set_cwd(&mut self, path: &Path) -> std::io::Result<()> {
        self.last_cwd = stdx::env::set_current_working_dir(path)?;
        self.clear_doc_relative_paths();
        self.file_watcher
            .reload(&self.config().file_watcher.clone());
        self.refresh_vcs_watches();
        Ok(())
    }

    pub fn get_last_cwd(&mut self) -> Option<&Path> {
        self.last_cwd.as_deref()
    }

    pub fn replace_quicklist(&mut self, entries: Vec<QuicklistEntry>) {
        self.quicklist.replace(entries);
    }

    pub fn jump_next_quicklist(&mut self, view_id: ViewId, count: usize, same_file: bool) -> bool {
        self.jump_quicklist(view_id, Direction::Forward, count, same_file)
    }

    pub fn jump_prev_quicklist(&mut self, view_id: ViewId, count: usize, same_file: bool) -> bool {
        self.jump_quicklist(view_id, Direction::Backward, count, same_file)
    }

    pub fn jump_forward(&mut self, view_id: ViewId, count: usize) {
        if let Some((doc_id, selection)) = view_mut!(self, view_id).jumps.forward(count).cloned() {
            self.jump_to(view_id, doc_id, selection);
        }
    }

    pub fn jump_backward(&mut self, view_id: ViewId, count: usize) {
        let view = view_mut!(self, view_id);
        let doc = doc_mut!(self, &view.doc);
        // `backward` may push the current selection (valid at the document's
        // current revision) onto the jumplist. Sync first so the view's
        // `doc_revisions` matches, otherwise that entry would be left ahead of
        // it and a later sync would map it out of bounds.
        view.sync_changes(doc);
        if let Some((doc_id, selection)) = view.jumps.backward(view_id, doc, count).cloned() {
            self.jump_to(view_id, doc_id, selection);
        }
    }

    fn jump_to(&mut self, view_id: ViewId, dest_doc_id: DocumentId, mut selection: Selection) {
        let view = view_mut!(self, view_id);
        let old_doc_id = view.doc;
        if old_doc_id != dest_doc_id {
            let new_doc = doc_mut!(self, &dest_doc_id);
            if let Some(transaction) = view.changes_to_sync(new_doc) {
                let text = new_doc.text().slice(..);
                selection = selection.map(transaction.changes()).ensure_invariants(text);
            }
            self.replace_document_in_view(view_id, dest_doc_id);
            dispatch(DocumentFocusLost {
                editor: self,
                doc: old_doc_id,
            });
        }
        let (view, doc) = current!(self);
        doc.set_selection(view_id, selection);
        view.ensure_cursor_in_view_center(doc, self.config.load().scrolloff);
    }

    fn jump_quicklist(
        &mut self,
        view_id: ViewId,
        direction: Direction,
        count: usize,
        same_file: bool,
    ) -> bool {
        let current_doc_id = self.tree.get(view_id).doc;
        let current_path = if same_file {
            self.documents
                .get(&current_doc_id)
                .and_then(|doc| doc.path())
        } else {
            None
        };

        let matched = match direction {
            Direction::Forward => {
                self.quicklist
                    .next_entry(count, current_doc_id, current_path, same_file)
            }
            Direction::Backward => {
                self.quicklist
                    .prev_entry(count, current_doc_id, current_path, same_file)
            }
        };

        let Some(QuicklistMatch { index, entry }) = matched else {
            return false;
        };

        if self.activate_quicklist_entry(view_id, &entry.clone(), Action::Replace) {
            self.quicklist.set_current(Some(index));
            true
        } else {
            false
        }
    }

    pub fn activate_quicklist_entry(
        &mut self,
        view_id: ViewId,
        entry: &QuicklistEntry,
        action: Action,
    ) -> bool {
        let doc_id = match &entry.target {
            QuicklistTarget::Path(path) => match self.open(path, action) {
                Ok(id) => id,
                Err(err) => {
                    self.set_error(|| {
                        format!("failed to open quicklist entry '{}': {err}", path.display())
                    });
                    return false;
                }
            },
            QuicklistTarget::Document(doc_id) => {
                if !self.documents.contains_key(doc_id) {
                    self.set_error(|| "The quicklist entry no longer points to an open document.");
                    return false;
                }
                self.switch(*doc_id, action);
                *doc_id
            }
        };

        match &entry.position {
            QuicklistPosition::None => {
                // TODO: extend quicklist capture for more pickers so fewer
                // entries need to fall back to the document's restored cursor.
                true
            }
            QuicklistPosition::Selection(selection) => {
                let scrolloff = self.config.load().scrolloff;
                let target_view_id = match action {
                    Action::Replace => view_id,
                    Action::HorizontalSplit | Action::VerticalSplit => self.tree.focus,
                    Action::Load => view_id,
                };
                let view = view_mut!(self, target_view_id);
                let doc = doc_mut!(self, &doc_id);
                let selection = selection.clone().ensure_invariants(doc.text().slice(..));
                doc.set_selection(view.id, selection);
                if action.align_view(view, doc.id()) {
                    view.ensure_cursor_in_view_center(doc, scrolloff);
                }
                true
            }
            QuicklistPosition::LineRange {
                start: line_start,
                end: line_end,
            } => {
                let (start, end) = {
                    let doc = doc_mut!(self, &doc_id);
                    let text = doc.text();

                    if *line_start >= text.len_lines() {
                        self.set_error(|| {
                            "The quicklist entry no longer points to a valid line in the file."
                        });
                        return false;
                    }
                    let start = text.line_to_char(*line_start);
                    let end = text.line_to_char((*line_end + 1).min(text.len_lines()));
                    (start, end)
                };

                let scrolloff = self.config.load().scrolloff;
                let target_view_id = match action {
                    Action::Replace => view_id,
                    Action::HorizontalSplit | Action::VerticalSplit => self.tree.focus,
                    Action::Load => view_id,
                };
                let view = view_mut!(self, target_view_id);
                let doc = doc_mut!(self, &doc_id);
                doc.set_selection(view.id, Selection::single(start, end));
                if action.align_view(view, doc.id()) {
                    view.ensure_cursor_in_view_center(doc, scrolloff);
                }
                true
            }
            QuicklistPosition::LineColRange {
                start_line,
                start_col,
                end_line,
                end_col,
            } => {
                let selection = {
                    let doc = doc_mut!(self, &doc_id);
                    let text = doc.text();

                    if *start_line >= text.len_lines() || *end_line >= text.len_lines() {
                        self.set_error(|| {
                            "The quicklist entry no longer points to a valid location in the file."
                        });
                        return false;
                    }

                    let start = text.line_to_char(*start_line).saturating_add(*start_col);
                    let end = text.line_to_char(*end_line).saturating_add(*end_col);
                    Selection::single(start, end).ensure_invariants(text.slice(..))
                };

                let scrolloff = self.config.load().scrolloff;
                let target_view_id = match action {
                    Action::Replace => view_id,
                    Action::HorizontalSplit | Action::VerticalSplit => self.tree.focus,
                    Action::Load => view_id,
                };
                let view = view_mut!(self, target_view_id);
                let doc = doc_mut!(self, &doc_id);
                doc.set_selection(view.id, selection);
                if action.align_view(view, doc.id()) {
                    view.ensure_cursor_in_view_center(doc, scrolloff);
                }
                true
            }
            QuicklistPosition::LspRange {
                range,
                offset_encoding,
            } => {
                let selection = {
                    let doc = doc_mut!(self, &doc_id);
                    let Some(range) = lsp_range_to_range(doc.text(), *range, *offset_encoding)
                    else {
                        self.set_error(|| {
                            "The quicklist entry no longer points to a valid location in the file."
                        });
                        return false;
                    };
                    Selection::single(range.head, range.anchor)
                };

                let scrolloff = self.config.load().scrolloff;
                let target_view_id = match action {
                    Action::Replace => view_id,
                    Action::HorizontalSplit | Action::VerticalSplit => self.tree.focus,
                    Action::Load => view_id,
                };
                let view = view_mut!(self, target_view_id);
                let doc = doc_mut!(self, &doc_id);
                doc.set_selection(view.id, selection);
                if action.align_view(view, doc.id()) {
                    view.ensure_cursor_in_view_center(doc, scrolloff);
                }
                true
            }
        }
    }
}

fn try_restore_indent(doc: &mut Document, view: &mut View) {
    use editor_core::{
        chars::char_is_whitespace,
        line_ending::{line_end_char_index, str_is_line_ending},
        unicode::segmentation::UnicodeSegmentation,
        Operation, Transaction,
    };

    fn inserted_a_new_blank_line(changes: &[Operation], pos: usize, line_end_pos: usize) -> bool {
        if let [Operation::Retain(move_pos), Operation::Insert(inserted_str), Operation::Retain(_)] =
            changes
        {
            let mut graphemes = inserted_str.graphemes(true);
            move_pos + inserted_str.len() == pos
                && graphemes.next().is_some_and(str_is_line_ending)
                && graphemes.all(|g| g.chars().all(char_is_whitespace))
                && pos == line_end_pos // ensure no characters exists after current position
        } else {
            false
        }
    }

    let doc_changes = doc.changes().changes();
    let text = doc.text().slice(..);
    let range = doc.selection(view.id).primary();
    let pos = range.cursor(text);
    let line_end_pos = line_end_char_index(&text, range.cursor_line(text));

    if inserted_a_new_blank_line(doc_changes, pos, line_end_pos) {
        // Removes tailing whitespaces for the primary selection only, preserving existing behavior
        let line_start_pos = text.line_to_char(range.cursor_line(text));
        let transaction =
            Transaction::change(doc.text(), [(line_start_pos, pos, None)].into_iter());
        doc.apply(&transaction, view.id);
    }
}

#[derive(Default)]
pub struct CursorCache(Cell<Option<Option<Position>>>);

impl CursorCache {
    pub fn get(&self, view: &View, doc: &Document) -> Option<Position> {
        if let Some(pos) = self.0.get() {
            return pos;
        }

        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);
        let res = view.screen_coords_at_pos(doc, text, cursor);
        self.set(res);
        res
    }

    pub fn set(&self, cursor_pos: Option<Position>) {
        self.0.set(Some(cursor_pos))
    }

    pub fn reset(&self) {
        self.0.set(None)
    }
}
