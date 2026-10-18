pub use crate::features::formatting::FormatterError;
pub(crate) mod history;
mod language_servers;
mod lifecycle;
mod mutation;
mod view_state;
use view_state::DocumentViewState;

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

mod editorconfig;
use editorconfig::EditorConfig;

mod file;
pub use file::{
    DocumentOpenError, DocumentSavedEvent, DocumentSavedEventFuture, DocumentSavedEventResult,
};
mod file_io;
use ::text_io::from_reader;

mod syntax_initialization;

use syntax_initialization::PendingSyntax;
pub use syntax_initialization::SyntaxRequest;

use anyhow::{bail, Error};
use arc_swap::access::DynAccess;
use arc_swap::ArcSwap;
use editor_core::auto_pairs::AutoPairs;
use editor_core::encoding::Encoding;
use language_config::LanguageServerFeature;
use snippets::{ActiveSnippet, SnippetRenderCtx};
use text_layout::doc_formatter::TextFormat;
use text_layout::text_annotations::Overlay;
use vcs::{DiffHandle, DiffProviderRegistry};

use ::parking_lot::Mutex;
use serde::de::{self, Deserialize, Deserializer};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::str::FromStr;
use std::sync::{Arc, Weak};

use crate::{
    events::SelectionDidChange,
    features::{
        code_lenses::DocumentCodeLenses, document_colors::DocumentColors,
        document_highlight::DocumentHighlightsState, document_links::DocumentLinks,
        document_symbols::DocumentSymbols, hover::HoverState, inlay_hints::InlayHintsState,
        navigation::NavigationState,
    },
    view::ViewPosition,
    DocumentId, View, ViewId,
};
use editor_config::Config;
use editor_core::{
    encoding,
    indent::{auto_detect_indent_style, IndentStyle},
    line_ending::auto_detect_line_ending,
    ChangeSet, LineEnding, Range, Rope, Selection, Transaction,
};

use language::{syntax, Syntax};
use language_config::LanguageConfiguration;
use theme::Theme;

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

impl Mode {
    pub fn cursor_shape(
        self,
        shapes: &editor_config::CursorShapeConfig,
    ) -> crate::graphics::CursorKind {
        match self {
            Self::Normal => shapes.normal,
            Self::Select => shapes.select,
            Self::Insert => shapes.insert,
        }
    }
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
#[derive(Debug)]
pub struct SavePoint {
    /// The view this savepoint is associated with
    pub view: ViewId,
    revert: Mutex<Transaction>,
}

impl SavePoint {
    /// Forward changes from this savepoint to the current document text.
    pub(crate) fn changes_since(&self, current: &editor_core::Rope) -> editor_core::ChangeSet {
        self.revert.lock().changes().invert(current)
    }
}

pub struct Document {
    redraw: event::Redraw,
    status: Option<event::status::StatusSender>,
    pub(crate) id: DocumentId,
    pub(crate) buffer: buffer::Buffer,
    view_states: HashMap<ViewId, DocumentViewState>,
    pub active_snippet: Option<ActiveSnippet>,

    // Each feature owns its cache, scheduling handle, and request controllers.
    // Dropping the document drops those controllers and cancels in-flight work.
    pub(crate) hover: HoverState,
    pub(crate) navigation: NavigationState,
    pub(crate) shell_filters: crate::features::shell::ShellFilters,
    pub(crate) rename: event::TaskController,
    pub(crate) inlay_hints: InlayHintsState,
    pub(crate) inline_completions: InlineCompletions,
    pub(crate) accepted_completions: crate::features::completion::AcceptedCompletions,
    pub(crate) document_colors: DocumentColors,
    pub(crate) code_lenses: DocumentCodeLenses,
    pub(crate) document_links: DocumentLinks,
    pub(crate) document_highlights: DocumentHighlightsState,
    pub(crate) code_action_hints: CodeActionHints,
    pub(crate) code_actions: crate::features::code_actions::CodeActionsState,
    pub(crate) document_symbols: DocumentSymbols,
    pub(crate) workspace_symbols: event::TaskController,

    file: file::FileState,

    pub restore_cursor: bool,

    /// Current indent style.
    pub indent_style: IndentStyle,
    editor_config: EditorConfig,
    /// The languages (dictionaries) to spell check this buffer against, or empty to disable spell
    /// checking. A word is flagged only when every dictionary rejects it.
    ///
    /// Resolved by [`Document::detect_spelling_languages`] from the manual override, the
    /// document's `.editorconfig` (`spelling_language = en_US`), or the resolved config.
    pub(crate) spelling_languages: Vec<language_config::SpellingLanguage>,
    /// A manual `:set-spelling-language` choice, which takes precedence over `.editorconfig` and
    /// config when resolving [`Document::spelling_languages`]. `None` means derive from those; an
    /// empty `Vec` forces spell checking off regardless of what they say.
    pub(crate) spelling_language_override: Option<Vec<language_config::SpellingLanguage>>,

    /// The document's default line ending.
    pub line_ending: LineEnding,

    pub(crate) syntax: Option<Syntax>,
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
    pub(crate) language: Option<Arc<LanguageConfiguration>>,

    pub(crate) selection_history: crate::editing::SelectionHistory,
    pub config: Arc<dyn DynAccess<Config>>,

    savepoints: Vec<Weak<SavePoint>>,

    pub(crate) modified_since_accessed: bool,

    pub(crate) diagnostics: ::diagnostics::DiagnosticStore,
    pub(crate) language_servers: HashMap<LanguageServerName, Arc<Client>>,
    opened_language_servers: HashSet<LanguageServerId>,

    diff_handle: Option<DiffHandle>,
    version_control_head: Option<Arc<ArcSwap<Box<str>>>>,
    pub(crate) blame: DocumentBlame,

    // when document was used for most-recent-used buffer picker
    pub focused_at: std::time::Instant,

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
            .field("buffer", &self.buffer)
            .field("views", &self.view_states)
            .field("text_annotations", &self.inlay_hints)
            .field("file", &self.file)
            .field("restore_cursor", &self.restore_cursor)
            .field("syntax", &self.syntax)
            .field("language", &self.language)
            .field("modified_since_accessed", &self.modified_since_accessed)
            .field("diagnostics", &self.diagnostics)
            // .field("language_server", &self.language_server)
            .finish()
    }
}

use lsp_client::{lsp, Client, LanguageServerId, LanguageServerName};

impl Document {
    pub fn from(
        text: Rope,
        encoding_with_bom_info: Option<(&'static Encoding, bool)>,
        config: Arc<dyn DynAccess<Config>>,
        syn_loader: Arc<ArcSwap<syntax::Loader>>,
    ) -> Self {
        let (encoding, has_bom) = encoding_with_bom_info.unwrap_or((encoding::UTF_8, false));
        let line_ending = config.load().files.default_line_ending.into();

        Self {
            id: DocumentId::default(),
            active_snippet: None,
            file: file::FileState::new(encoding, has_bom),
            buffer: buffer::Buffer::new(text),
            view_states: HashMap::default(),
            inlay_hints: Default::default(),
            hover: Default::default(),
            navigation: Default::default(),
            shell_filters: Default::default(),
            rename: Default::default(),
            inline_completions: Default::default(),
            accepted_completions: Default::default(),
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
            selection_history: Default::default(),
            diagnostics: Default::default(),
            savepoints: Vec::new(),
            modified_since_accessed: false,
            language_servers: HashMap::new(),
            opened_language_servers: HashSet::new(),
            diff_handle: None,
            redraw: event::Redraw::default(),
            status: None,
            blame: Default::default(),
            config,
            version_control_head: None,
            focused_at: std::time::Instant::now(),
            document_highlights: Default::default(),
            code_action_hints: Default::default(),
            code_actions: Default::default(),
            document_links: Default::default(),
            document_colors: Default::default(),
            code_lenses: Default::default(),
            document_symbols: Default::default(),
            workspace_symbols: Default::default(),
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

    /// Detect the language and initialize syntax without delaying file loading.
    pub(crate) fn detect_language(&mut self, loader: &Arc<syntax::Loader>) {
        self.invalidate_language_features();
        self.language = self.detect_language_config(loader);
        self.initialize_syntax(loader.clone());
    }

    /// Detect the programming language based on the file type.
    pub fn detect_language_config(
        &self,
        loader: &syntax::Loader,
    ) -> Option<Arc<language_config::LanguageConfiguration>> {
        if self.is_binary() {
            return None;
        }
        let language = loader
            .language_for_filename(self.path()?)
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
            auto_detect_indent_style(self.buffer.text()).unwrap_or_else(|| {
                self.language_config()
                    .and_then(|config| config.indent.as_ref())
                    .map_or(DEFAULT_INDENT, |config| IndentStyle::from_str(&config.unit))
            })
        };
        if let Some(line_ending) = self
            .editor_config
            .line_ending
            .or_else(|| auto_detect_line_ending(self.buffer.text()))
        {
            self.line_ending = line_ending;
        }
        self.detect_spelling_languages();
    }

    /// Resolves the languages this document is spell-checked against, in precedence order: a manual
    /// `:set-spelling-language` override, then the `.editorconfig` `spelling_language`, then the
    /// resolved config (`languages.toml` over `[editor.spelling]`). Re-run when any of these change.
    pub fn detect_spelling_languages(&mut self) {
        if self.is_binary() {
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
            && let Some(path) = self.path()
        {
            self.editor_config = EditorConfig::find(path);
        }
    }

    /// Set the language and load its syntax immediately for an explicit language command.
    /// Automatic file loading uses [`Self::detect_language`] to initialize in the background.
    pub(crate) fn set_language(
        &mut self,
        language_config: Option<Arc<LanguageConfiguration>>,
        loader: &syntax::Loader,
    ) {
        self.pending_syntax = None;
        self.invalidate_language_features();
        self.language = language_config;
        self.syntax = self.language.as_ref().and_then(|config| {
            let language = loader.language_for_name(config.language_id.as_str())?;
            Syntax::new(self.buffer.text().slice(..), language, loader)
                .map_err(|err| {
                    if err != syntax::HighlighterError::NoRootConfig {
                        log::warn!("Error building syntax for '{}': {err}", self.display_name());
                    }
                })
                .ok()
        });
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

    /// Initialize this document/view relationship without resetting existing state.
    pub fn ensure_view_init(&mut self, view_id: ViewId) {
        if !self.view_states.contains_key(&view_id) {
            self.reset_selection(view_id);
        }

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
        self.view_states.remove(&view_id);
        self.inlay_hints.remove_view(view_id);
        self.hover.remove_view(view_id);
        self.code_actions.remove_view(view_id);
        self.navigation.remove_view(view_id);
        self.shell_filters.remove_view(view_id);
        self.document_links.cancel_navigation_for(view_id);
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

    /// Get the current revision number
    pub fn get_current_revision(&self) -> usize {
        self.buffer.current_revision()
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

    /// Resolved dictionaries after applying document, language, and editor settings.
    pub fn spelling_languages(&self) -> &[language_config::SpellingLanguage] {
        &self.spelling_languages
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
        self.buffer.version()
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
        if self.is_binary() {
            self.diff_handle = None;
        } else {
            match providers.get_diff_base(&path, trust_full) {
                Some(base) => self.set_diff_base(base),
                None => self.diff_handle = None,
            }
        }
        self.version_control_head = providers.get_current_head_name(&path, trust_full);
        if self.config.load().inline_blame.show != editor_config::InlineBlameShow::Never
            && let Some(handler) = self.blame.handler().cloned()
        {
            handler.schedule_refresh(self.id());
        }
    }

    /// Initialize or update the differ for this document with a new base.
    pub fn set_diff_base(&mut self, diff_base: Vec<u8>) {
        if self.is_binary() {
            self.diff_handle = None;
            return;
        }
        match from_reader(&mut diff_base.as_slice(), Some(self.encoding())) {
            Ok((diff_base, ..)) => {
                if let Some(differ) = &self.diff_handle {
                    differ.update_diff_base(diff_base);
                    return;
                }
                self.diff_handle = Some(DiffHandle::new(
                    diff_base,
                    self.buffer.text().clone(),
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
        self.buffer.changes()
    }

    /// Whether this file-backed document should display and maintain breadcrumbs.
    #[inline]
    pub fn breadcrumb_enabled(&self) -> bool {
        self.path().is_some() && self.config.load().breadcrumb.enable
    }

    /// Capture immutable text and its identity for saving or background work.
    pub fn snapshot(&self) -> buffer::Snapshot {
        self.buffer.snapshot()
    }

    #[inline]
    pub fn text(&self) -> &Rope {
        self.buffer.text()
    }

    #[inline]
    pub fn selection(&self, view_id: ViewId) -> &Selection {
        self.try_selection(view_id)
            .expect("view must be initialized for this document")
    }

    /// Selection only when this document has state for the given view.
    pub fn try_selection(&self, view_id: ViewId) -> Option<&Selection> {
        Some(&self.view_states.get(&view_id)?.selection)
    }

    pub fn selections(&self) -> impl ExactSizeIterator<Item = &Selection> {
        self.view_states.values().map(|state| &state.selection)
    }

    pub fn view_ids(&self) -> impl ExactSizeIterator<Item = ViewId> + '_ {
        self.view_states.keys().copied()
    }

    fn view_state_mut(&mut self, view_id: ViewId) -> &mut DocumentViewState {
        let text = self.buffer.text().slice(..);
        self.view_states
            .entry(view_id)
            .or_insert_with(|| DocumentViewState::new(Selection::point(0).ensure_invariants(text)))
    }

    pub(crate) fn get_view_offset(&self, view_id: ViewId) -> Option<ViewPosition> {
        Some(self.view_states.get(&view_id)?.position)
    }

    pub fn view_offset(&self, view_id: ViewId) -> ViewPosition {
        self.get_view_offset(view_id)
            .expect("view must be initialized for this document")
    }

    pub fn set_view_offset(&mut self, view_id: ViewId, new_offset: ViewPosition) {
        self.view_state_mut(view_id).position = new_offset;
    }

    // transact(Fn) ?

    // -- LSP methods

    #[inline]
    pub fn identifier(&self) -> lsp::TextDocumentIdentifier {
        lsp::TextDocumentIdentifier::new(self.url().unwrap())
    }

    pub fn versioned_identifier(&self) -> lsp::VersionedTextDocumentIdentifier {
        lsp::VersionedTextDocumentIdentifier::new(self.url().unwrap(), self.buffer.version())
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

    /// Get the document's auto pairs. If the document has a recognized
    /// language config with auto pairs configured, returns that;
    /// otherwise, falls back to the global auto pairs config. If the global
    /// config is false, then ignore language settings.
    pub fn auto_pairs<'a>(
        &'a self,
        global_config: Option<&'a AutoPairs>,
        loader: &'a syntax::Loader,
        selection: &Selection,
    ) -> Option<&'a AutoPairs> {
        global_config?;

        self.syntax
            .as_ref()
            .and_then(|syntax| {
                let selection = selection.primary();
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
        self.view_state_mut(view_id).jump_labels = labels;
    }

    pub fn remove_jump_labels(&mut self, view_id: ViewId) {
        if let Some(state) = self.view_states.get_mut(&view_id) {
            state.jump_labels.clear();
        }
    }

    pub(crate) fn jump_labels(&self, view_id: ViewId) -> Option<&[Overlay]> {
        let labels = &self.view_states.get(&view_id)?.jump_labels;
        (!labels.is_empty()).then_some(labels.as_slice())
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
    #[tokio::test]
    async fn document_view_state_is_preserved_on_init_and_removed_together() {
        let mut doc = feature_document();
        let mut views = slotmap::SlotMap::<ViewId, ()>::with_key();
        let first = views.insert(());
        let second = views.insert(());
        doc.ensure_view_init(first);
        doc.ensure_view_init(second);
        let selection = Selection::single(2, 3);
        let position = ViewPosition {
            anchor: 3,
            horizontal_offset: 2,
            vertical_offset: 1,
        };
        doc.set_selection(first, selection.clone());
        doc.set_view_offset(first, position);
        doc.set_jump_labels(first, vec![Overlay::new(4, "a")]);
        doc.ensure_view_init(first);
        assert_eq!(doc.selection(first), &selection);
        assert_eq!(doc.view_offset(first), position);
        assert_eq!(doc.jump_labels(first).unwrap().len(), 1);
        let other_selection = doc.selection(second).clone();

        doc.remove_view(first);
        assert!(doc.try_selection(first).is_none());
        assert!(doc.get_view_offset(first).is_none());
        assert!(doc.jump_labels(first).is_none());
        assert_eq!(doc.selection(second), &other_selection);
        assert_eq!(doc.view_ids().collect::<Vec<_>>(), [second]);
        doc.remove_jump_labels(first);
        assert!(doc.try_selection(first).is_none());
        doc.ensure_view_init(first);
        assert_eq!(doc.selection(first).primary(), doc.origin());
        assert_eq!(doc.view_offset(first), ViewPosition::default());
        assert!(doc.jump_labels(first).is_none());
    }

    #[tokio::test]
    async fn document_edits_map_each_views_selection_and_viewport_anchor() {
        let mut doc = feature_document();
        let mut views = slotmap::SlotMap::<ViewId, ()>::with_key();
        let first = views.insert(());
        let second = views.insert(());
        for (view, anchor) in [(first, 2), (second, 7)] {
            doc.ensure_view_init(view);
            doc.set_selection(view, Selection::single(anchor, anchor + 1));
            doc.set_view_offset(
                view,
                ViewPosition {
                    anchor,
                    horizontal_offset: 3,
                    vertical_offset: 1,
                },
            );
        }
        let edit = Transaction::change(doc.text(), [(0, 0, Some("界x".into()))].into_iter());
        doc.apply(edit.changes()).unwrap();
        for (view, anchor) in [(first, 4), (second, 9)] {
            assert_eq!(doc.selection(view), &Selection::single(anchor, anchor + 1));
            assert_eq!(
                doc.view_offset(view),
                ViewPosition {
                    anchor,
                    horizontal_offset: 3,
                    vertical_offset: 1
                }
            );
        }
    }
}
