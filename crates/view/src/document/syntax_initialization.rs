//! Syntax snapshots and publication rules, coordinated by the document's syntax handler.

use std::sync::Arc;

use event::{cancelable_future, TaskController, TaskHandle};
use language::{syntax, Language, Syntax};
use tokio::sync::Semaphore;

use super::Document;

// Bound CPU work when opening many buffers, including workers compiling shared queries.
static WORKERS: Semaphore = Semaphore::const_new(2);

pub(super) struct PendingSyntax {
    language: Language,
    loader: Arc<syntax::Loader>,
    controller: TaskController,
}

/// A cancelable snapshot for background syntax initialization.
pub struct SyntaxRequest {
    snapshot: buffer::Snapshot,
    language: Language,
    loader: Arc<syntax::Loader>,
    cancel: TaskHandle,
}

/// The tree [`SyntaxRequest::compute_first`] can show before injections are compiled.
pub struct FirstSyntax {
    /// `None` when the language has no usable syntax configuration.
    pub syntax: Option<Syntax>,
    /// Injected languages whose queries still need compiling; empty when final.
    pub missing: Vec<Language>,
}

fn syntax_or_none(
    result: Result<Syntax, syntax::HighlighterError>,
) -> anyhow::Result<Option<Syntax>> {
    match result {
        Ok(syntax) => Ok(Some(syntax)),
        // Missing/invalid configuration has already been reported by the loader.
        Err(syntax::HighlighterError::NoRootConfig) => Ok(None),
        Err(err) => Err(anyhow::anyhow!("{err}")),
    }
}

impl SyntaxRequest {
    /// Wait until this snapshot is superseded or its document cancels the request.
    pub async fn canceled(&self) {
        self.cancel.canceled().await;
    }

    /// Compute off-thread, or return `None` when canceled. A successful result
    /// may contain no tree when the language has no usable syntax configuration.
    pub async fn compute(&self) -> Option<anyhow::Result<Option<Syntax>>> {
        self.run(|text, language, loader| {
            syntax_or_none(Syntax::new(text.slice(..), language, loader))
        })
        .await
    }

    /// First stage of [`SyntaxHandler`](crate::features::syntax::SyntaxHandler)
    /// initialization: a tree that compiles only the root language's queries.
    ///
    /// Injected languages that were not compiled yet are skipped and returned. When none
    /// are, the tree is final and goes to [`Self::complete`]; otherwise it is shown with
    /// [`Self::publish_first`] while [`Self::compute_injections`] compiles the rest.
    pub async fn compute_first(&self) -> Option<anyhow::Result<FirstSyntax>> {
        self.run(|text, language, loader| {
            match Syntax::new_with_compiled(text.slice(..), language, loader) {
                Ok((syntax, missing)) => Ok(FirstSyntax {
                    syntax: Some(syntax),
                    missing,
                }),
                Err(err) => syntax_or_none(Err(err)).map(|syntax| FirstSyntax {
                    syntax,
                    missing: Vec::new(),
                }),
            }
        })
        .await
    }

    /// Compile the injected languages a first tree skipped and parse again.
    pub async fn compute_injections(
        &self,
        missing: Vec<Language>,
    ) -> Option<anyhow::Result<Option<Syntax>>> {
        self.run(move |text, language, loader| {
            loader.compile_syntax_configs(&missing);
            syntax_or_none(Syntax::new(text.slice(..), language, loader))
        })
        .await
    }

    async fn run<R: Send + 'static>(
        &self,
        work: impl FnOnce(&editor_core::Rope, Language, &syntax::Loader) -> anyhow::Result<R>
            + Send
            + 'static,
    ) -> Option<anyhow::Result<R>> {
        cancelable_future(
            async {
                let permit = WORKERS.acquire().await?;
                let text = self.snapshot.text().clone();
                let language = self.language;
                let loader = self.loader.clone();
                let cancel = self.cancel.clone();
                tokio::task::spawn_blocking(move || {
                    // A running blocking task retains its permit even after cancellation.
                    let _permit = permit;
                    if cancel.is_canceled() {
                        // Publication checks cancellation first, so this is never shown.
                        anyhow::bail!("syntax initialization was canceled");
                    }
                    work(&text, language, &loader)
                })
                .await?
            },
            &self.cancel,
        )
        .await
    }

    /// Show a first tree on the editor thread, keeping initialization pending until
    /// its injections are published by [`Self::complete`].
    pub fn publish_first(&self, doc: &mut Document, syntax: Syntax) {
        if !self.is_current(doc) {
            return;
        }
        if !self.snapshot.is_current(&doc.buffer) {
            doc.restart_syntax_initialization(self.loader.clone());
            return;
        }
        self.loader.refresh_highlights();
        doc.syntax = Some(syntax);
    }

    /// Whether this request still belongs to the document's initialization.
    fn is_current(&self, doc: &Document) -> bool {
        !self.cancel.is_canceled() && Arc::ptr_eq(&self.loader, &doc.syntax_loader)
    }

    /// Publish on the editor thread. Superseded requests are ignored, and edits
    /// made during compilation start a fresh snapshot using the cached queries.
    /// Returns whether initialization finished and dependent features can refresh.
    pub fn complete(&self, doc: &mut Document, result: anyhow::Result<Option<Syntax>>) -> bool {
        if !self.is_current(doc) {
            return false;
        }
        if !self.snapshot.is_current(&doc.buffer) {
            // A first tree has been updated with those edits and stays visible meanwhile.
            doc.restart_syntax_initialization(self.loader.clone());
            return false;
        }
        doc.pending_syntax = None;
        match result {
            Ok(syntax) => {
                self.loader.refresh_highlights();
                doc.syntax = syntax;
            }
            Err(err) => log::error!(
                "Syntax initialization failed for '{}': {err}",
                doc.display_name()
            ),
        }
        true
    }
}

impl Document {
    /// Cancel the current snapshot, leaving initialization pending for a later request.
    pub fn cancel_syntax_request(&mut self) {
        if let Some(pending) = self.pending_syntax.as_mut() {
            pending.controller.cancel();
        }
    }

    /// Capture the current text if initialization has not already been scheduled.
    pub fn syntax_request(&mut self) -> Option<SyntaxRequest> {
        let pending = self.pending_syntax.as_mut()?;
        if pending.controller.is_running() {
            return None;
        }
        Some(SyntaxRequest {
            snapshot: self.buffer.snapshot(),
            language: pending.language,
            loader: pending.loader.clone(),
            cancel: pending.controller.restart(),
        })
    }

    pub(crate) fn initialize_syntax(&mut self, loader: Arc<syntax::Loader>) {
        self.syntax = None;
        self.syntax_loader = loader.clone();
        self.pending_syntax = self.language.as_ref().and_then(|config| {
            let language = loader.language_for_name(config.language_id.as_str())?;
            Some(PendingSyntax {
                language,
                loader,
                controller: TaskController::new(),
            })
        });
        if self.pending_syntax.is_some()
            && let Some(handler) = self.syntax_handler.clone()
        {
            handler.request(self);
        }
    }

    /// Whether the document is waiting for its initial syntax tree, including a first
    /// tree that is shown while its injected languages compile.
    pub fn is_syntax_pending(&self) -> bool {
        self.pending_syntax.is_some()
    }

    pub(super) fn parse_syntax(&self, loader: &syntax::Loader) -> Option<Syntax> {
        let config = self.language.as_ref()?;
        let language = loader.language_for_name(config.language_id.as_str())?;
        Syntax::new(self.buffer.text().slice(..), language, loader)
            .map_err(|err| {
                if err != syntax::HighlighterError::NoRootConfig {
                    log::warn!("Error building syntax for '{}': {err}", self.display_name());
                }
            })
            .ok()
    }

    /// Take a fresh snapshot after edits, keeping any first tree visible meanwhile.
    fn restart_syntax_initialization(&mut self, loader: Arc<syntax::Loader>) {
        let shown = self.syntax.take();
        self.initialize_syntax(loader);
        if self.pending_syntax.is_some() {
            self.syntax = shown;
        }
    }

    /// Finish initialization on demand for commands that require a syntax tree.
    pub(crate) fn finish_syntax_initialization(&mut self) -> bool {
        let Some(mut pending) = self.pending_syntax.take() else {
            return false;
        };
        pending.controller.cancel();
        // Completing the same language snapshot must preserve view features
        // created while initialization was pending (for example manual folds).
        self.syntax = self.parse_syntax(&pending.loader);
        pending.loader.refresh_highlights();
        true
    }
}

#[cfg(test)]
mod tests {
    use arc_swap::ArcSwap;
    use editor_core::Rope;
    use editor_core::{Selection, Transaction};

    use super::*;
    use crate::{
        events::{DocumentDidChange, SelectionDidChange},
        ViewId,
    };
    use editor_config::Config;

    fn loader() -> Arc<syntax::Loader> {
        Arc::new(
            syntax::Loader::new(
                toml::from_str(
                    r#"
            [[language]]
            name = "json"
            scope = "source.json"
            file-types = ["json"]

            [[language]]
            name = "toml"
            scope = "source.toml"
            file-types = ["toml"]

            [[language]]
            name = "missing"
            scope = "source.missing"
            file-types = ["missing"]
            grammar = "missing-startup-test-grammar"
        "#,
                )
                .unwrap(),
                loader::syntax::Resources::default(),
            )
            .unwrap(),
        )
    }

    fn document(loader: &Arc<syntax::Loader>) -> Document {
        Document::from(
            Rope::from_str("{}"),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            loader.clone(),
        )
    }

    fn detect_language(doc: &mut Document, name: &str, loader: &Arc<syntax::Loader>) {
        doc.set_path(Some(std::path::Path::new(&format!("test.{name}"))));
        doc.detect_language(loader);
    }

    async fn finish(doc: &mut Document) {
        let request = doc.syntax_request().unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), request.compute())
            .await
            .expect("syntax initialization did not complete")
            .unwrap();
        assert!(request.complete(doc, result));
    }

    #[tokio::test]
    async fn publishes_syntax_after_initialization() {
        let loader = loader();
        let mut doc = document(&loader);
        detect_language(&mut doc, "json", &loader);

        assert!(doc.is_syntax_pending());
        assert!(doc.syntax().is_none());
        finish(&mut doc).await;
        assert!(!doc.is_syntax_pending());
        assert_eq!(doc.syntax().unwrap().tree().root_node().byte_range(), 0..2);
    }

    #[tokio::test]
    async fn selected_language_uses_the_current_loaders_parser_order() {
        let original = loader();
        let mut doc = document(&original);
        let json = original.language_for_name("json").unwrap();
        let config = original.language(json).config().clone();
        let current = Arc::new(
            syntax::Loader::new(
                toml::from_str(
                    r#"
                    [[language]]
                    name = "toml"
                    scope = "source.toml"
                    file-types = ["toml"]
                    [[language]]
                    name = "json"
                    scope = "source.json"
                    file-types = ["json"]
                    "#,
                )
                .unwrap(),
                loader::syntax::Resources::default(),
            )
            .unwrap(),
        );
        let expected = current.language_for_name("json").unwrap();
        assert_ne!(json, expected);

        doc.set_language(Some(config), &current);
        assert_eq!(doc.syntax().unwrap().root_language(), expected);

        doc.initialize_syntax(current);
        finish(&mut doc).await;
        assert_eq!(doc.syntax().unwrap().root_language(), expected);
    }

    #[tokio::test]
    async fn canceled_initialization_can_be_requested_again() {
        let loader = loader();
        let mut doc = document(&loader);
        detect_language(&mut doc, "json", &loader);
        let request = doc.syntax_request().unwrap();

        doc.cancel_syntax_request();
        request.canceled().await;
        assert!(request.compute().await.is_none());
        assert!(doc.is_syntax_pending());
        finish(&mut doc).await;
        assert!(doc.syntax().is_some());
    }

    #[tokio::test]
    async fn unregistered_documents_keep_syntax_resources_across_registry_changes() {
        event::register_event::<DocumentDidChange>();
        event::register_event::<SelectionDidChange>();
        let resources = loader();
        let registry = ArcSwap::new(resources.clone());
        let mut doc = document(&registry.load_full());
        detect_language(&mut doc, "json", &resources);
        let request = doc.syntax_request().unwrap();

        // Previews and standalone documents are not part of the editor's reload loop.
        registry.store(Arc::new(syntax::Loader::default()));
        let result = request.compute().await.unwrap();
        assert!(request.complete(&mut doc, result));
        assert!(std::ptr::eq(doc.syntax_loader(), resources.as_ref()));

        let transaction = Transaction::change(
            doc.text(),
            [(1, 1, Some("\"key\": true".into()))].into_iter(),
        );
        doc.apply(transaction.changes()).unwrap();
        let root = doc.syntax().unwrap().tree().root_node();
        assert_eq!(root.kind(), "document");
        assert_eq!(root.byte_range(), 0..doc.text().len() as u32);
        assert_eq!(doc.text().to_string(), "{\"key\": true}");
    }

    #[tokio::test]
    async fn loader_reload_discards_a_snapshot_before_publication() {
        let old_loader = loader();
        let mut doc = document(&old_loader);
        detect_language(&mut doc, "json", &old_loader);
        let request = doc.syntax_request().unwrap();
        let result = request.compute().await.unwrap();

        doc.detect_language(&loader());
        assert!(!request.complete(&mut doc, result));
        assert!(doc.syntax().is_none());
        finish(&mut doc).await;
        assert!(doc.syntax().is_some());
    }

    #[tokio::test]
    async fn edits_during_initialization_are_parsed_before_publication() {
        event::register_event::<DocumentDidChange>();
        event::register_event::<SelectionDidChange>();
        let loader = loader();
        let mut doc = document(&loader);
        detect_language(&mut doc, "json", &loader);

        let request = doc.syntax_request().unwrap();
        let view = ViewId::default();
        doc.set_selection(view, Selection::point(1));
        let transaction = Transaction::change(
            doc.text(),
            [(1, 1, Some("\"updated\": true".into()))].into_iter(),
        );
        crate::editing::apply(&mut doc, &transaction, view).unwrap();
        assert!(doc.syntax().is_none());

        let result = request.compute().await.unwrap();
        assert!(!request.complete(&mut doc, result));
        assert!(doc.syntax().is_none());
        finish(&mut doc).await;
        let root = doc.syntax().unwrap().tree().root_node();
        assert_eq!(root.byte_range(), 0..doc.text().len() as u32);
        assert_eq!(doc.text().to_string(), "{\"updated\": true}");
    }

    #[tokio::test]
    async fn language_and_loader_changes_replace_pending_syntax() {
        let old_loader = loader();
        let mut doc = document(&old_loader);
        detect_language(&mut doc, "json", &old_loader);

        let request = doc.syntax_request().unwrap();
        let result = request.compute().await.unwrap();
        let new_loader = loader();
        detect_language(&mut doc, "toml", &new_loader);
        assert!(!request.complete(&mut doc, result));
        finish(&mut doc).await;
        assert_eq!(
            doc.syntax().unwrap().root_language(),
            new_loader.language_for_name("toml").unwrap()
        );
    }

    #[tokio::test]
    async fn switching_to_plain_text_cancels_initialization() {
        let loader = loader();
        let mut doc = document(&loader);
        detect_language(&mut doc, "json", &loader);
        let request = doc.syntax_request().unwrap();
        doc.set_language(None, &loader);
        assert!(request.compute().await.is_none());

        assert!(!doc.is_syntax_pending());
        assert!(doc.syntax().is_none());
    }

    #[tokio::test]
    async fn closing_a_document_cancels_its_request() {
        let loader = loader();
        let mut doc = document(&loader);
        detect_language(&mut doc, "json", &loader);
        let request = doc.syntax_request().unwrap();
        drop(doc);
        assert!(request.compute().await.is_none());
    }

    /// A doc comment and a format string: Rust's two usual injections.
    const RUST: &str =
        "/// Returns **one**.\nfn one() -> u8 {\n    println!(\"{}\", 1);\n    1\n}\n";

    fn rust_document() -> (Document, Arc<syntax::Loader>) {
        let loader = Arc::new(language::loader::default_lang_loader(
            loader::syntax::Resources::default(),
        ));
        let mut doc = Document::from(
            Rope::from_str(RUST),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            loader.clone(),
        );
        detect_language(&mut doc, "rs", &loader);
        (doc, loader)
    }

    /// Layers containing a byte of the doc comment's text.
    fn doc_comment_layers(doc: &Document) -> usize {
        doc.syntax().unwrap().layers_for_byte_range(5, 5).count()
    }

    fn insert_line(doc: &mut Document) {
        event::register_event::<DocumentDidChange>();
        event::register_event::<SelectionDidChange>();
        let transaction =
            Transaction::change(doc.text(), [(0, 0, Some("use a;\n".into()))].into_iter());
        doc.apply(transaction.changes()).unwrap();
    }

    #[tokio::test]
    async fn first_tree_is_shown_while_injected_languages_compile() {
        let (mut doc, _loader) = rust_document();
        let request = doc.syntax_request().unwrap();
        let first = request.compute_first().await.unwrap().unwrap();
        assert!(!first.missing.is_empty());

        request.publish_first(&mut doc, first.syntax.unwrap());
        assert!(doc.is_syntax_pending());
        assert_eq!(doc_comment_layers(&doc), 1);

        let result = request.compute_injections(first.missing).await.unwrap();
        assert!(request.complete(&mut doc, result));
        assert!(!doc.is_syntax_pending());
        assert!(doc_comment_layers(&doc) > 1);
    }

    #[tokio::test]
    async fn edits_after_the_first_tree_keep_it_until_a_fresh_snapshot_completes() {
        let (mut doc, _loader) = rust_document();
        let request = doc.syntax_request().unwrap();
        let first = request.compute_first().await.unwrap().unwrap();
        request.publish_first(&mut doc, first.syntax.unwrap());
        insert_line(&mut doc);
        let len = doc.text().len() as u32;
        assert_eq!(
            doc.syntax().unwrap().tree().root_node().byte_range(),
            0..len
        );

        let result = request.compute_injections(first.missing).await.unwrap();
        assert!(!request.complete(&mut doc, result));
        // The stale result is dropped, but the edited first tree stays visible.
        assert!(doc.is_syntax_pending());
        assert_eq!(
            doc.syntax().unwrap().tree().root_node().byte_range(),
            0..len
        );

        // Queries are compiled by now, so the fresh snapshot is final at once.
        let request = doc.syntax_request().unwrap();
        let first = request.compute_first().await.unwrap().unwrap();
        assert!(first.missing.is_empty());
        assert!(request.complete(&mut doc, Ok(first.syntax)));
        assert!(!doc.is_syntax_pending());
        assert_eq!(
            doc.syntax().unwrap().tree().root_node().byte_range(),
            0..len
        );
    }

    #[tokio::test]
    async fn a_stale_first_tree_is_not_shown() {
        let (mut doc, _loader) = rust_document();
        let request = doc.syntax_request().unwrap();
        let first = request.compute_first().await.unwrap().unwrap();
        insert_line(&mut doc);

        request.publish_first(&mut doc, first.syntax.unwrap());
        assert!(doc.syntax().is_none());
        assert!(doc.is_syntax_pending());
        // The restart superseded the old request.
        assert!(request.compute_injections(first.missing).await.is_none());
        finish(&mut doc).await;
        assert_eq!(
            doc.syntax().unwrap().tree().root_node().byte_range(),
            0..doc.text().len() as u32
        );
    }

    #[tokio::test]
    async fn languages_without_injections_are_final_after_the_first_stage() {
        let loader = loader();
        let mut doc = document(&loader);
        detect_language(&mut doc, "json", &loader);
        let request = doc.syntax_request().unwrap();
        let first = request.compute_first().await.unwrap().unwrap();
        assert!(first.missing.is_empty());
        assert!(request.complete(&mut doc, Ok(first.syntax)));
        assert!(doc.syntax().is_some());

        detect_language(&mut doc, "missing", &loader);
        let request = doc.syntax_request().unwrap();
        let first = request.compute_first().await.unwrap().unwrap();
        assert!(first.syntax.is_none() && first.missing.is_empty());
    }

    #[tokio::test]
    async fn missing_grammar_finishes_without_retrying() {
        let loader = loader();
        let mut doc = document(&loader);
        detect_language(&mut doc, "missing", &loader);
        finish(&mut doc).await;

        assert!(doc.syntax().is_none());
        assert!(!doc.is_syntax_pending());
    }
}
