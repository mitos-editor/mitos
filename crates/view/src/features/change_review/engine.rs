//! Review policy and immutable inputs, shared by hunk and repository comparisons.

use std::{path::Path, sync::Arc};

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

    pub fn same(&self, other: &Self) -> bool {
        self.current(other) && self.engine == other.engine
    }

    fn current(&self, other: &Self) -> bool {
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
    pub(super) before: Rope,
    pub(super) after: Rope,
    pub(super) line: Option<usize>,
    pub(super) settings: Settings,
}

impl ReviewSource {
    pub fn engine(&self) -> DiffEngine {
        self.settings.engine
    }

    pub fn compute(&self, engine: Option<DiffEngine>, canceled: &dyn Fn() -> bool) -> Review {
        let engine = engine.unwrap_or(self.engine());
        if engine == DiffEngine::Line {
            return Review::new(self.before.clone(), self.after.clone(), self.line);
        }
        match self.provider() {
            Ok(provider) => Review::with_provider(
                self.before.clone(),
                self.after.clone(),
                self.line,
                provider.as_ref(),
                canceled,
            ),
            Err(reason) => {
                let mut review = Review::new(self.before.clone(), self.after.clone(), self.line);
                if engine != DiffEngine::Auto
                    || !matches!(reason.0, "no language configured" | "grammar unavailable")
                {
                    review.fallback = Some(reason);
                }
                review
            }
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
