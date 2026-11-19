//! Immutable review sources and their shared comparison and highlighting caches.

use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use editor_core::Rope;
use language::{Language, Loader, Syntax};
use text_diff::{Review, ReviewLine, ReviewLineKind};

use crate::Editor;

#[derive(Clone)]
pub(super) struct LanguageContext {
    loader: Arc<Loader>,
    language: Option<Language>,
}

impl LanguageContext {
    pub fn capture(editor: &Editor, path: &Path) -> Self {
        let loader = editor.syn_loader.load_full();
        let language = editor.document_by_path(path).map_or_else(
            || loader.language_for_filename(path),
            |doc| {
                doc.language_name()
                    .and_then(|name| loader.language_for_name(name))
            },
        );
        Self { loader, language }
    }

    pub(super) fn historical(editor: &Editor, path: &Path) -> Self {
        let loader = editor.syn_loader.load_full();
        let language = loader.language_for_filename(path);
        Self { loader, language }
    }

    pub(super) fn current(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.loader, &other.loader) && self.language == other.language
    }

    pub fn is_current(&self, editor: &Editor, path: &Path) -> bool {
        self.current(&Self::capture(editor, path))
    }
}

/// Exact review snapshots and the language resources captured for source highlighting.
pub struct ReviewSource {
    before: Rope,
    after: Rope,
    line: Option<usize>,
    before_context: Option<LanguageContext>,
    context: LanguageContext,
    review: OnceLock<Arc<Review>>,
    syntax: OnceLock<SourceSyntax>,
}

struct SourceSyntax {
    before: Option<Syntax>,
    after: Option<Syntax>,
}

impl ReviewSource {
    pub(super) fn loader(&self) -> Arc<Loader> {
        self.context.loader.clone()
    }

    pub(super) fn with_before_context(mut self, context: LanguageContext) -> Self {
        self.before_context = Some(context);
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
        let context = if before {
            self.before_context.as_ref().unwrap_or(&self.context)
        } else {
            &self.context
        };
        let language = context
            .language
            .map(|lang| context.loader.language(lang).config().clone());
        let syntax = self.syntax.get().and_then(|syntax| {
            if before {
                syntax.before.clone()
            } else {
                syntax.after.clone()
            }
        });
        doc.install_snapshot_syntax(language, context.loader.clone(), syntax);
    }

    pub(super) fn new(
        before: Rope,
        after: Rope,
        line: Option<usize>,
        context: LanguageContext,
    ) -> Self {
        Self {
            before,
            after,
            line,
            before_context: None,
            context,
            review: OnceLock::new(),
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
            self.context.loader.clone(),
        );
        self.initialize_document(&mut doc, false);
        let _ = doc.conflicts.set(model);
        doc
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
        let parse = |text: &Rope, context: &LanguageContext| {
            context
                .language
                .and_then(|language| Syntax::new(text.slice(..), language, &context.loader).ok())
        };
        let before = parse(
            &self.before,
            self.before_context.as_ref().unwrap_or(&self.context),
        );
        if canceled() {
            return;
        }
        let after = parse(&self.after, &self.context);
        if !canceled() {
            // A theme switch may have happened while queries were compiling.
            self.context.loader.refresh_highlights();
            let _ = self.syntax.set(SourceSyntax { before, after });
        }
    }

    pub fn syntax_loader(&self) -> &Loader {
        &self.context.loader
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

    /// Cache one review of these snapshots. Syntax emphasis uses the same trees as highlighting.
    pub fn compute(&self, canceled: &dyn Fn() -> bool) -> Arc<Review> {
        if canceled() {
            return Arc::new(Review::notice("Comparison canceled"));
        }
        if let Some(review) = self.review.get() {
            return review.clone();
        }
        let mut review = Review::new(self.before.clone(), self.after.clone(), self.line);
        if !review.truncated {
            self.prepare_syntax(canceled);
            if let Some(syntax) = self.syntax.get()
                && let (Some(before), Some(after)) = (&syntax.before, &syntax.after)
            {
                review = review.with_syntax(
                    &self.before,
                    &self.after,
                    before.tree(),
                    after.tree(),
                    canceled,
                );
            }
        }
        let review = Arc::new(review);
        if !canceled() {
            let _ = self.review.set(review.clone());
        }
        review
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canceled_preparation_does_not_cache_a_partial_review() -> anyhow::Result<()> {
        let loader = Arc::new(Loader::new(
            toml::from_str(
                r#"
                [[language]]
                name = "json"
                scope = "source.json"
                file-types = ["json"]
            "#,
            )?,
            loader::syntax::Resources::default(),
        )?);
        let language = loader.language_for_name("json");
        let source = ReviewSource::new(
            Rope::from("{\"value\": 1}\n"),
            Rope::from("{\"value\": 2}\n"),
            None,
            LanguageContext { loader, language },
        );
        let checks = std::cell::Cell::new(0);
        source.compute(&|| {
            checks.set(checks.get() + 1);
            checks.get() >= 3
        });
        assert!(source.review.get().is_none());
        let review = source.compute(&|| false);
        assert!(review.lines.iter().any(|line| line.changes.is_some()));
        let changed = review
            .lines
            .iter()
            .find(|row| row.kind == ReviewLineKind::Added)
            .unwrap();
        let revision = source.syntax_for_row(changed).unwrap().1.revision();
        assert!(Arc::ptr_eq(&review, &source.compute(&|| false)));
        assert_eq!(
            source.syntax_for_row(changed).unwrap().1.revision(),
            revision
        );
        Ok(())
    }
}
