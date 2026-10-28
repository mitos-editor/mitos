//! Review policy and immutable inputs, shared by hunk and repository comparisons.

use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use editor_core::Rope;
use language::{Language, Loader, Syntax};
use language_config::DiffEngine;
use text_diff::{Review, ReviewLine, ReviewLineKind, ReviewProvider, ReviewUnavailable};

use crate::Editor;

#[derive(Clone)]
pub(super) struct Settings {
    loader: Arc<Loader>,
    language: Option<Language>,
    pub engine: DiffEngine,
}

impl Settings {
    pub fn capture(editor: &Editor, path: &Path) -> Self {
        let loader = editor.syn_loader.load_full();
        let language = editor.document_by_path(path).map_or_else(
            || loader.language_for_filename(path),
            |doc| {
                doc.language_name()
                    .and_then(|name| loader.language_for_name(name))
            },
        );
        let configured = language
            .and_then(|lang| loader.language(lang).config().diff_engine)
            .unwrap_or(editor.config().diff.engine);
        Self {
            loader,
            language,
            engine: configured,
        }
    }

    pub(super) fn historical(editor: &Editor, path: &Path) -> Self {
        let mut settings = Self::capture(editor, path);
        settings.language = settings.loader.language_for_filename(path);
        settings.engine = settings
            .language
            .and_then(|lang| settings.loader.language(lang).config().diff_engine)
            .unwrap_or(editor.config().diff.engine);
        settings
    }

    pub(super) fn current(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.loader, &other.loader)
            && self.language == other.language
            && self.engine == other.engine
    }

    pub fn is_current(&self, editor: &Editor, path: &Path) -> bool {
        self.current(&Self::capture(editor, path))
    }
}

/// The exact snapshots behind an open review. Language resources and comparison policy
/// belong to the captured editor configuration.
pub struct ReviewSource {
    before: Rope,
    after: Rope,
    line: Option<usize>,
    before_settings: Option<Settings>,
    settings: Settings,
    line_review: OnceLock<Arc<Review>>,
    syntax_review: OnceLock<Arc<Review>>,
    syntax: OnceLock<SourceSyntax>,
}

struct SourceSyntax {
    before: Option<Syntax>,
    after: Option<Syntax>,
}

impl ReviewSource {
    pub fn before(&self) -> &Rope {
        &self.before
    }
    pub(super) fn loader(&self) -> Arc<Loader> {
        self.settings.loader.clone()
    }

    pub(super) fn with_before_settings(mut self, settings: Settings) -> Self {
        self.before_settings = Some(settings);
        self
    }
    pub(super) fn text(&self, before: bool) -> &Rope {
        if before {
            &self.before
        } else {
            &self.after
        }
    }
    pub(super) fn initialize_document(&self, doc: &mut crate::Document, before: bool) {
        let settings = if before {
            self.before_settings.as_ref().unwrap_or(&self.settings)
        } else {
            &self.settings
        };
        let language = settings
            .language
            .map(|lang| settings.loader.language(lang).config().clone());
        let syntax = self.syntax.get().and_then(|syntax| {
            if before {
                syntax.before.clone()
            } else {
                syntax.after.clone()
            }
        });
        doc.install_snapshot_syntax(language, settings.loader.clone(), syntax);
    }

    pub(super) fn new(before: Rope, after: Rope, line: Option<usize>, settings: Settings) -> Self {
        Self {
            before,
            after,
            line,
            before_settings: None,
            settings,
            line_review: OnceLock::new(),
            syntax_review: OnceLock::new(),
            syntax: OnceLock::new(),
        }
    }

    /// Unregistered source snapshot for a structured picker preview. It has no
    /// filesystem identity, write history, or attached language server.
    pub fn conflict_document(
        &self,
        config: Arc<dyn arc_swap::access::DynAccess<editor_config::Config>>,
        model: Arc<crate::features::conflicts::ConflictPresentation>,
    ) -> crate::Document {
        let mut doc = crate::Document::from(
            self.after.clone(),
            None,
            config,
            self.settings.loader.clone(),
        );
        self.initialize_document(&mut doc, false);
        let _ = doc.conflicts.set(model);
        doc
    }

    pub fn engine(&self) -> DiffEngine {
        self.settings.engine
    }

    /// The immutable after-side used to validate navigation into a live document.
    pub fn after(&self) -> &Rope {
        &self.after
    }

    /// Parse the immutable sides off-thread using the ordinary language loader,
    /// injection support, and interactive parse budget. Never parse unified rows
    /// as source code. Canceled work must not populate this shared cache.
    pub(super) fn prepare_syntax(&self, canceled: &dyn Fn() -> bool) {
        if self.syntax.get().is_some() || canceled() {
            return;
        }
        let parse = |text: &Rope, settings: &Settings| {
            settings
                .language
                .and_then(|language| Syntax::new(text.slice(..), language, &settings.loader).ok())
        };
        let before = parse(
            &self.before,
            self.before_settings.as_ref().unwrap_or(&self.settings),
        );
        if canceled() {
            return;
        }
        let after = parse(&self.after, &self.settings);
        if !canceled() {
            // A theme switch may have happened while queries were compiling.
            self.settings.loader.refresh_highlights();
            let _ = self.syntax.set(SourceSyntax { before, after });
        }
    }

    pub fn syntax_loader(&self) -> &Loader {
        &self.settings.loader
    }

    /// Source coordinates and complete syntax context behind a projected row.
    /// Header/notice rows and sources without a usable grammar have no syntax.
    pub fn syntax_for_row(&self, row: &ReviewLine) -> Option<(&Rope, &Syntax, usize)> {
        let syntax = self.syntax.get()?;
        match row.kind {
            ReviewLineKind::Removed => Some((&self.before, syntax.before.as_ref()?, row.before?)),
            ReviewLineKind::Added | ReviewLineKind::Context => {
                Some((&self.after, syntax.after.as_ref()?, row.after?))
            }
            ReviewLineKind::Header | ReviewLineKind::Notice => None,
        }
    }

    /// Retain at most two bounded models per source. Canceled syntax work never enters the cache.
    pub fn compute(&self, engine: Option<DiffEngine>, canceled: &dyn Fn() -> bool) -> Arc<Review> {
        if canceled() {
            return Arc::new(Review::notice("Comparison canceled"));
        }
        let engine = engine.unwrap_or(self.engine());
        let line = self.line_review.get_or_init(|| {
            Arc::new(Review::new(
                self.before.clone(),
                self.after.clone(),
                self.line,
            ))
        });
        if engine == DiffEngine::Line || canceled() {
            return line.clone();
        }
        if self
            .before_settings
            .as_ref()
            .is_some_and(|before| before.language != self.settings.language)
        {
            let mut review = line.as_ref().clone();
            if engine == DiffEngine::Syntax {
                review.fallback = Some(ReviewUnavailable("source languages differ"));
            }
            return Arc::new(review);
        }
        let syntax = self.syntax_review.get().cloned().unwrap_or_else(|| {
            let review = Arc::new(match self.provider() {
                Ok(provider) => line.as_ref().clone().annotated(
                    &self.before,
                    &self.after,
                    provider.as_ref(),
                    canceled,
                ),
                Err(reason) => {
                    let mut review = line.as_ref().clone();
                    review.fallback = Some(reason);
                    review
                }
            });
            if !canceled() {
                let _ = self.syntax_review.set(review.clone());
            }
            review
        });
        if engine == DiffEngine::Auto
            && syntax.fallback.is_some_and(|reason| {
                matches!(reason.0, "no language configured" | "grammar unavailable")
            })
        {
            line.clone()
        } else {
            syntax
        }
    }

    #[cfg(feature = "syntax-diff")]
    fn provider(&self) -> Result<Box<dyn ReviewProvider>, ReviewUnavailable> {
        self.settings
            .language
            .ok_or(ReviewUnavailable("no language configured"))
            .and_then(|language| {
                self.settings
                    .loader
                    .grammar(language)
                    .map_err(|_| ReviewUnavailable("grammar could not be loaded"))?
                    .ok_or(ReviewUnavailable("grammar unavailable"))
            })
            .map(|grammar| {
                Box::new(syntax_diff::SyntaxDiff::new(grammar)) as Box<dyn ReviewProvider>
            })
    }

    #[cfg(not(feature = "syntax-diff"))]
    fn provider(&self) -> Result<Box<dyn ReviewProvider>, ReviewUnavailable> {
        Err(ReviewUnavailable(
            "syntax provider not included in this build",
        ))
    }
}
