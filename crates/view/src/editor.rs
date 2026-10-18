//! Editor state and coordination. Document/view lifecycle and filesystem operations
//! live in private implementation modules behind the same public `Editor` API.

mod config;
mod document_settings;
mod document_store;
mod documents;
use document_store::Documents;
pub use text_layout::DisplayPosition;
mod filesystem;
pub use filesystem::FileOperation;
mod language_servers;
mod mode;
mod registers;
use crate::{
    document::{DocumentSavedEventResult, Mode},
    events::DocumentFocusLost,
    features::diagnostics::LspDiagnostics,
    graphics::{CursorKind, Rect},
    handlers::Handlers,
    info::Info,
    input::KeyEvent,
    quicklist::{Quicklist, QuicklistEntry, QuicklistMatch, QuicklistPosition, QuicklistTarget},
    register::Registers,
    save::WriteQueue,
    tree::{self, Tree},
    Document, DocumentId, View, ViewId,
};
use editor_config::Config;
use event::dispatch;
use theme::Theme;
use vcs::DiffProviderRegistry;
use workspace::trust::WorkspaceTrust;

use futures_util::StreamExt;
use lsp_client::{Call, LanguageServerId};

use std::{
    borrow::Cow,
    cell::Cell,
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use tokio::{
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    time::{sleep, Duration, Instant, Sleep},
};

use anyhow::bail;

use crate::file_watcher::Watcher;
use ::diagnostics::Severity;
use dap::{self as dap, registry::DebugAdapterId};
use editor_core::{auto_pairs::AutoPairs, movement::Direction, Range, Selection};

use language::syntax;
use lsp_client::{lsp, util::lsp_range_to_range};

use arc_swap::{
    access::{DynAccess, DynGuard},
    ArcSwap,
};

pub const DIR_STACK_CAP: usize = 10;

pub use crate::debugger::Breakpoint;

pub struct Editor {
    /// Current editing mode.
    mode: Mode,
    pub tree: Tree,
    pub documents: Documents,

    writes: WriteQueue,
    file_operation: event::TaskController,

    pub count: Option<std::num::NonZeroUsize>,
    pub selected_register: Option<char>,
    pub registers: Registers,
    pub macro_recording: Option<(char, Vec<KeyEvent>)>,
    pub macro_replaying: Vec<char>,
    pub language_servers: crate::language_servers::Registry,
    initialized_language_servers: HashSet<LanguageServerId>,
    pub(crate) diagnostics: LspDiagnostics,
    pub diff_providers: DiffProviderRegistry,

    pub(crate) debugger: crate::debugger::DebuggerState,

    pub syn_loader: Arc<ArcSwap<syntax::Loader>>,
    pub theme_loader: Arc<theme::Loader>,
    /// last_theme is used for theme previews. We store the current theme here,
    /// and if previewing is cancelled, we can return to it.
    pub last_theme: Option<Theme>,
    /// The currently applied editor theme. While previewing a theme, the previewed theme
    /// is set here.
    pub theme: Theme,

    pub quicklist: Quicklist,

    pub status_msg: Option<(Cow<'static, str>, Severity)>,
    pub autoinfo: Option<Info>,

    pub config: Arc<dyn DynAccess<Config>>,
    pub auto_pairs: Option<AutoPairs>,

    pub idle_timer: Pin<Box<Sleep>>,
    redraw_timer: Pin<Box<Sleep>>,
    last_motion: Option<Motion>,
    /// Most recent visible inline blame mode, restored when toggling it back on.
    pub last_inline_blame_show: Option<editor_config::InlineBlameShow>,
    pub last_cwd: Option<PathBuf>,
    pub dir_stack: VecDeque<PathBuf>,

    pub exit_code: i32,

    pub config_events: (UnboundedSender<ConfigEvent>, UnboundedReceiver<ConfigEvent>),
    pub needs_redraw: bool,
    /// Editor-owned services; replacements must preserve document bindings.
    pub(crate) handlers: Handlers,

    pub mouse_down_range: Option<Range>,
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
    pub cursor_cache: CursorCache,
    pub file_watcher: Watcher,
    pub workspace_trust: WorkspaceTrust,
}

pub type Motion = Box<dyn Fn(&mut Editor)>;

#[derive(Debug)]
pub enum EditorEvent {
    DocumentSaved(DocumentSavedEventResult),
    ConfigEvent(ConfigEvent),
    ReloadConfirmation(crate::features::auto_reload::ReloadRequest),
    WorkspaceTrust(crate::features::workspace_trust::TrustRequest),
    SignatureHelp(crate::features::signature_help::SignatureHelpUpdate),
    Hover(crate::features::hover::HoverUpdate),
    BlameDetails(crate::features::blame::BlameDetailsUpdate),
    Navigation(crate::features::navigation::NavigationUpdate),
    Completion(crate::features::completion::CompletionUpdate),
    LanguageServerMessage((LanguageServerId, Call)),
    DebuggerEvent((DebugAdapterId, dap::ClientEvent)),
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
        let language_servers = crate::language_servers::Registry::new(syn_loader.clone());
        let conf = config.load();
        let auto_pairs = (&conf.auto_pairs).into();

        // Initialize file watcher and diff providers
        let file_watcher = Watcher::new(&conf.files.watch, handlers.callbacks.clone());
        let diff_providers = DiffProviderRegistry::default();

        // HAXX: offset the render area height by 1 to account for prompt/commandline
        area.height -= 1;

        Self {
            mode: Mode::Normal,
            tree: Tree::new(area),
            documents: Documents::new(),
            writes: WriteQueue::default(),
            file_operation: event::TaskController::default(),
            count: None,
            selected_register: None,
            macro_recording: None,
            macro_replaying: Vec::new(),
            theme: theme_loader.default(),
            language_servers,
            initialized_language_servers: HashSet::new(),
            diagnostics: LspDiagnostics::new(),
            diff_providers,
            debugger: crate::debugger::DebuggerState::default(),
            syn_loader,
            theme_loader,
            last_theme: None,
            quicklist: Quicklist::default(),
            registers: Registers::new(crate::clipboard::Clipboard::native(Box::new(
                arc_swap::access::Map::new(Arc::clone(&config), |config: &Config| {
                    &config.clipboard.provider
                }),
            ))),
            status_msg: None,
            autoinfo: None,
            idle_timer: Box::pin(sleep(conf.idle_timeout)),
            redraw_timer: Box::pin(sleep(Duration::MAX)),
            last_motion: None,
            last_inline_blame_show: None,
            last_cwd: None,
            config,
            auto_pairs,
            exit_code: 0,
            config_events: unbounded_channel(),
            needs_redraw: false,
            handlers,
            mouse_down_range: None,
            cursor_cache: CursorCache::default(),
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

    fn _refresh(&mut self) {
        let config = self.config();

        // Reset the inlay hints annotations *before* updating the views, that way we ensure they
        // will disappear during the `.sync_change(doc)` call below.
        //
        // We can't simply check this config when rendering because inlay hints are only parts of
        // the possible annotations, and others could still be active, so we need to selectively
        // drop the inlay hints.
        if !config.lsp.inlay_hints.enable {
            for doc in self.documents_mut() {
                doc.reset_all_inlay_hints();
            }
        }

        for (view, _) in self.tree.views_mut() {
            let doc = doc_mut!(self, &view.doc);
            view.sync_changes(doc);
            view.gutters = config.gutters.clone();
            view.ensure_cursor_in_view(doc, config.scroll.padding)
        }
    }

    /// Apply one independent undoable edit to any open document, including hidden
    /// buffers. The expected version rejects asynchronous results built from stale
    /// text, even when an intervening edit preserved the document's length.
    /// Validation and empty edits leave pending typing groups untouched.
    pub fn apply_document_changes(
        &mut self,
        id: DocumentId,
        expected_version: i32,
        changes: &editor_core::ChangeSet,
    ) -> Result<(), crate::editing::EditError> {
        use crate::editing::EditError;

        let doc = self
            .documents
            .get_mut(&id)
            .ok_or(EditError::DocumentClosed(id))?;
        if doc.version() != expected_version {
            return Err(EditError::DocumentChanged {
                expected: expected_version,
                actual: doc.version(),
            });
        }
        doc.validate_changes(changes)?;
        if changes.is_empty() {
            return Ok(());
        }
        doc.commit_changes();
        self.sync_document_views(id);
        let doc = self.documents.get_mut(&id).unwrap();
        doc.apply(changes)?;
        doc.commit_changes();
        self.sync_document_views(id);
        Ok(())
    }

    /// Synchronize each displaying split through the document's committed history.
    pub(crate) fn sync_document_views(&mut self, id: DocumentId) {
        let Some(doc) = self.documents.get_mut(&id) else {
            return;
        };
        for (view, _) in self.tree.views_mut() {
            if view.doc == id {
                view.sync_changes(doc);
            }
        }
    }

    pub fn save<P: Into<PathBuf>>(
        &mut self,
        doc_id: DocumentId,
        path: Option<P>,
        force: bool,
    ) -> anyhow::Result<()> {
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
                        lsp::FileChangeType::CREATED
                    } else {
                        lsp::FileChangeType::CHANGED
                    },
                );
            }
            res
        };

        self.writes.enqueue(doc_id, future)
    }

    /// Whether accepted writes still await completion in the event loop or a flush.
    pub fn has_pending_writes(&self) -> bool {
        self.writes.has_pending()
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
        crate::editing::commit(doc, view);
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
            view: prev_id,
        })
        .report_to(self.status_sender());
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
        view.ensure_cursor_in_view(doc, config.scroll.padding)
    }

    /// Gets the primary cursor position in screen coordinates,
    /// or `None` if the primary cursor is not visible on screen.
    pub fn cursor(&self) -> (Option<DisplayPosition>, CursorKind) {
        let config = self.config();
        let (view, doc) = current_ref!(self);
        if let Some(mut pos) = self.cursor_cache.get(view, doc) {
            let inner = view.inner_area(doc);
            pos.col += inner.x as usize;
            pos.row += inner.y as usize;
            let cursorkind = self.mode.cursor_shape(&config.cursor.shape);
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
        if let Some(update) = crate::features::blame::next_update(self) {
            return EditorEvent::BlameDetails(update);
        }
        if let Some(update) = crate::features::completion::next_update(self) {
            return EditorEvent::Completion(update);
        }
        if let Some(update) = crate::features::navigation::next_update(self) {
            return EditorEvent::Navigation(update);
        }
        if let Some(update) = crate::features::hover::next_update(self) {
            return EditorEvent::Hover(update);
        }
        if let Some(update) = crate::features::signature_help::next_update(self) {
            return EditorEvent::SignatureHelp(update);
        }
        if let Some(request) = crate::features::auto_reload::next_reload_request(self) {
            return EditorEvent::ReloadConfirmation(request);
        }
        if let Some(request) = crate::features::workspace_trust::next_request(self) {
            return EditorEvent::WorkspaceTrust(request);
        }
        let redraw = self.redraw().clone();
        // the loop only runs once or twice and would be better implemented with a recursion + const generic
        // however due to limitations with async functions that can not be implemented right now
        loop {
            tokio::select! {
                biased;

                Some(event) = self.writes.next() => {
                    return EditorEvent::DocumentSaved(event)
                }
                Some(config_event) = self.config_events.1.recv() => {
                    return EditorEvent::ConfigEvent(config_event)
                }
                Some(message) = self.handlers.status_messages.recv() => {
                    let severity = match message.severity {
                        event::status::Severity::Hint => Severity::Hint,
                        event::status::Severity::Info => Severity::Info,
                        event::status::Severity::Warning => Severity::Warning,
                        event::status::Severity::Error => Severity::Error,
                    };
                    self.status_msg = Some((message.message, severity));
                    redraw.request();
                }
                Some(message) = self.language_servers.incoming.next() => {
                    return EditorEvent::LanguageServerMessage(message)
                }
                Some(event) = self.debugger.incoming.next() => {
                    return EditorEvent::DebuggerEvent(event)
                }

                _ = redraw.requested() => {
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
        let mut first_error = None;
        while self.writes.has_pending() {
            let Some(result) = self.writes.next().await else {
                bail!("save queue closed with unfinished writes");
            };
            match result {
                Ok(event) => self.complete_document_write(&event),
                Err(error) => {
                    self.set_error(|| error.to_string());
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The shared successful-write completion path, including write-and-quit.
    /// The event describes the written text, not edits made after submission.
    pub fn complete_document_write(&mut self, event: &crate::document::DocumentSavedEvent) {
        if let Some(doc) = self.document_mut(event.doc_id) {
            log::debug!(
                "document {:?} saved with revision {}",
                doc.path(),
                event.revision
            );
            doc.set_last_saved_revision(event.revision, event.save_time);
            self.set_doc_path(event.doc_id, &event.path);
        }
    }

    /// Debugger sessions and breakpoints owned by this editor.
    pub fn debugger(&self) -> &crate::debugger::DebuggerState {
        &self.debugger
    }

    /// Change debugger state through its session and breakpoint operations.
    pub fn debugger_mut(&mut self) -> &mut crate::debugger::DebuggerState {
        &mut self.debugger
    }

    pub fn current_stack_frame(&self) -> Option<&dap::StackFrame> {
        self.debugger.current_stack_frame()
    }

    /// Include the workspace and repositories for every open document, including linked worktrees.
    pub fn refresh_vcs_watches(&mut self) {
        let mut paths = Vec::new();
        if self.config().files.watch.watch_vcs {
            let root = workspace::find_workspace().0;
            let trusted = self
                .workspace_trust
                .query(&root, workspace::trust::TrustQuery::Git)
                .is_trusted();
            paths.extend(self.diff_providers.get_watched_paths(&root, trusted));
            for doc in self.documents.values() {
                let Some(path) = doc.path().and_then(|path| path.parent()) else {
                    continue;
                };
                let trust_full = self
                    .workspace_trust
                    .query(doc.workspace_root(), workspace::trust::TrustQuery::Git)
                    .is_trusted();
                paths.extend(self.diff_providers.get_watched_paths(path, trust_full));
            }
        }
        self.file_watcher.set_extra_watched_paths(paths);
    }

    /// The frontend destination bound to this editor during construction.
    /// Redraw destination shared by this editor's frontend and background services.
    pub fn redraw(&self) -> &event::Redraw {
        self.handlers.callbacks.redraw()
    }

    pub fn status_sender(&self) -> &event::status::StatusSender {
        &self.handlers.status
    }

    pub fn callback_sender(&self) -> &crate::callbacks::EditorCallbackSender {
        &self.handlers.callbacks
    }

    pub fn set_cwd(&mut self, path: &Path) -> std::io::Result<()> {
        self.last_cwd = stdx::env::set_current_working_dir(path)?;
        self.clear_doc_relative_paths();
        self.file_watcher.reload(&self.config().files.watch.clone());
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
                view: view_id,
            })
            .report_to(self.status_sender());
        }
        let (view, doc) = current!(self);
        doc.set_selection(view_id, selection);
        view.ensure_cursor_in_view_center(doc, self.config.load().scroll.padding);
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
                let scrolloff = self.config.load().scroll.padding;
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

                let scrolloff = self.config.load().scroll.padding;
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

                let scrolloff = self.config.load().scroll.padding;
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

                let scrolloff = self.config.load().scroll.padding;
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

#[derive(Default)]
pub struct CursorCache(Cell<Option<Option<DisplayPosition>>>);

impl CursorCache {
    pub fn get(&self, view: &View, doc: &Document) -> Option<DisplayPosition> {
        if let Some(pos) = self.0.get() {
            return pos;
        }

        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);
        let res = view.screen_coords_at_pos(doc, text, cursor);
        self.set(res);
        res
    }

    pub fn set(&self, cursor_pos: Option<DisplayPosition>) {
        self.0.set(Some(cursor_pos))
    }

    pub fn reset(&self) {
        self.0.set(None)
    }
}
