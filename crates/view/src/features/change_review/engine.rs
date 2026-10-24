//! Review policy and immutable inputs, shared by hunk and repository comparisons.

use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use editor_core::Rope;
use language::{Language, Loader};
use language_config::DiffEngine;
use text_diff::{Review, ReviewProvider, ReviewUnavailable};

use crate::Editor;

#[derive(Clone)]
pub(super) struct Settings {
    loader: Arc<Loader>,
    language: Option<Language>,
    configured: DiffEngine,
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
            configured,
            engine: configured,
        }
    }

    pub(super) fn current(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.loader, &other.loader)
            && self.language == other.language
            && self.configured == other.configured
    }

    pub fn is_current(&self, editor: &Editor, path: &Path) -> bool {
        self.current(&Self::capture(editor, path))
    }
}

/// The exact snapshots behind an open review. Switching its presentation never rereads Git or
/// changes the document. Language resources and policy belong to the captured editor setup.
pub struct ReviewSource {
    before: Rope,
    after: Rope,
    line: Option<usize>,
    settings: Settings,
    line_review: OnceLock<Arc<Review>>,
    syntax_review: OnceLock<Arc<Review>>,
}

impl ReviewSource {
    pub(super) fn new(before: Rope, after: Rope, line: Option<usize>, settings: Settings) -> Self {
        Self {
            before,
            after,
            line,
            settings,
            line_review: OnceLock::new(),
            syntax_review: OnceLock::new(),
        }
    }

    pub fn engine(&self) -> DiffEngine {
        self.settings.engine
    }

    /// The immutable after-side used to validate navigation into a live document.
    pub fn after(&self) -> &Rope {
        &self.after
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
