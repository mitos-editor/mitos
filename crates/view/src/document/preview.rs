//! Cached file text without editor services, undo history, or document feature state.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use arc_swap::access::DynAccess;
use editor_config::Config;
use editor_core::{indent::IndentStyle, Rope};
use language::{syntax, Syntax};
use language_config::LanguageConfiguration;
use text_layout::doc_formatter::TextFormat;
use theme::Theme;

use super::{
    file, settings::TextSettings, syntax_initialization::PendingSyntax, EditorConfig, SyntaxRequest,
};

pub struct PreviewText {
    snapshot: buffer::Snapshot,
    path: PathBuf,
    config: Arc<dyn DynAccess<Config>>,
    editor_config: EditorConfig,
    pub(crate) language: Option<Arc<LanguageConfiguration>>,
    syntax_loader: Arc<syntax::Loader>,
    syntax: Option<Syntax>,
    pending_syntax: Option<PendingSyntax>,
    diagnostics: diagnostics::DiagnosticStore,
    conflicts: OnceLock<crate::features::conflicts::ConflictPresentation>,
    indent_style: IndentStyle,
    binary: bool,
}

pub(super) fn detect_language(
    path: &Path,
    text: &Rope,
    loader: &syntax::Loader,
) -> Option<Arc<LanguageConfiguration>> {
    let language = loader
        .language_for_filename(path)
        .or_else(|| loader.language_for_shebang(text.slice(..)))?;
    Some(loader.language(language).config().clone())
}

impl PreviewText {
    pub fn open(
        path: &Path,
        config: Arc<dyn DynAccess<Config>>,
        syntax_loader: Arc<syntax::Loader>,
    ) -> Result<Self, file::DocumentOpenError> {
        let (decoded, editor_config) = file::load_text(path, None, &config.load())?;
        let path = stdx::path::canonicalize(path);
        let language = (!decoded.binary)
            .then(|| detect_language(&path, &decoded.text, &syntax_loader))
            .flatten();
        let indent_style = TextSettings {
            config: &config.load(),
            // Match file loading: detect indentation before selecting the language.
            language: None,
            editor_config: &editor_config,
        }
        .indent_style(&decoded.text);
        let pending_syntax = PendingSyntax::new(language.as_deref(), syntax_loader.clone());
        Ok(Self {
            snapshot: buffer::Snapshot::new(decoded.text),
            path,
            config,
            editor_config,
            language,
            syntax_loader,
            syntax: None,
            pending_syntax,
            diagnostics: Default::default(),
            conflicts: Default::default(),
            indent_style,
            binary: decoded.binary,
        })
    }

    pub fn text(&self) -> &Rope {
        self.snapshot.text()
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn is_binary(&self) -> bool {
        self.binary
    }
    pub fn language_config(&self) -> Option<&LanguageConfiguration> {
        self.language.as_deref()
    }
    pub fn syntax_loader(&self) -> &syntax::Loader {
        &self.syntax_loader
    }
    pub fn syntax(&self) -> Option<&Syntax> {
        self.syntax.as_ref()
    }
    pub fn is_syntax_pending(&self) -> bool {
        self.pending_syntax.is_some()
    }
    pub fn diagnostics(&self) -> &[diagnostics::Diagnostic] {
        self.diagnostics.as_slice()
    }
    pub fn indent_style(&self) -> IndentStyle {
        self.indent_style
    }
    pub fn tab_width(&self) -> usize {
        self.settings(&self.config.load()).tab_width()
    }
    pub fn text_format(&self, width: u16, theme: Option<&Theme>) -> TextFormat {
        self.settings(&self.config.load()).text_format(width, theme)
    }
    fn settings<'a>(&'a self, config: &'a Config) -> TextSettings<'a> {
        TextSettings {
            config,
            language: self.language_config(),
            editor_config: &self.editor_config,
        }
    }

    pub fn refresh_diagnostics(&mut self, editor: &crate::Editor) {
        self.diagnostics
            .replace(editor.diagnostics_for_preview(self), &[], None);
    }

    pub fn cancel_syntax_request(&mut self) {
        if let Some(pending) = &mut self.pending_syntax {
            pending.cancel();
        }
    }
    pub fn syntax_request(&mut self) -> Option<SyntaxRequest> {
        self.pending_syntax.as_mut()?.request(self.snapshot.clone())
    }
    pub fn complete_syntax(
        &mut self,
        request: &SyntaxRequest,
        result: anyhow::Result<Option<Syntax>>,
    ) -> bool {
        if !request.matches(&self.snapshot, &self.syntax_loader) {
            return false;
        }
        self.pending_syntax = None;
        match result {
            Ok(syntax) => {
                self.syntax_loader.refresh_highlights();
                self.syntax = syntax;
            }
            Err(error) => log::error!(
                "Syntax initialization failed for '{}': {error}",
                self.path.display()
            ),
        }
        true
    }

    /// Keep compatible syntax across file rewrites; replacing the owner cancels old work.
    pub fn refresh(&mut self, mut refreshed: Self) {
        let same_language = match (&self.language, &refreshed.language) {
            (Some(previous), Some(current)) => Arc::ptr_eq(previous, current),
            (None, None) => true,
            _ => false,
        };
        if same_language
            && Arc::ptr_eq(&self.syntax_loader, &refreshed.syntax_loader)
            && let Some(mut syntax) = self.syntax.take()
        {
            let transaction = text_diff::compare_ropes(self.text(), refreshed.text());
            match syntax.update(
                self.text().slice(..),
                refreshed.text().slice(..),
                transaction.changes(),
                &refreshed.syntax_loader,
            ) {
                Ok(()) => {
                    refreshed.syntax = Some(syntax);
                    refreshed.pending_syntax = None;
                }
                Err(error) => log::error!("Failed to update preview syntax: {error}"),
            }
        }
        *self = refreshed;
    }

    pub fn conflict_marker_highlights(
        &self,
        visible: std::ops::Range<usize>,
        theme: &Theme,
    ) -> editor_core::highlight::OverlayHighlights {
        self.conflicts
            .get_or_init(|| crate::features::conflicts::ConflictPresentation::new(self.text()))
            .marker_highlights(self.text(), visible, theme)
    }
}
