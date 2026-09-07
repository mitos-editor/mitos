//! Syntax snapshots and publication rules; completion is delivered by the UI job queue.

use std::sync::Arc;

use editor_core::{syntax, Language, Rope, Syntax};
use event::{cancelable_future, TaskController, TaskHandle};
use tokio::sync::Semaphore;

use super::Document;

// Bound CPU work when opening many buffers, including workers compiling shared queries.
static WORKERS: Semaphore = Semaphore::const_new(2);

pub(super) struct PendingSyntax {
    loader: Arc<syntax::Loader>,
    controller: TaskController,
}

/// A cancelable snapshot for background syntax initialization.
pub struct SyntaxRequest {
    text: Rope,
    version: i32,
    language: Language,
    loader: Arc<syntax::Loader>,
    cancel: TaskHandle,
}

impl SyntaxRequest {
    /// Compute off-thread, or return `None` when canceled. A successful result
    /// may contain no tree when the language has no usable syntax configuration.
    pub async fn compute(&self) -> Option<anyhow::Result<Option<Syntax>>> {
        cancelable_future(
            async {
                let permit = WORKERS.acquire().await?;
                let text = self.text.clone();
                let language = self.language;
                let loader = self.loader.clone();
                let cancel = self.cancel.clone();
                tokio::task::spawn_blocking(move || {
                    // A running blocking task retains its permit even after cancellation.
                    let _permit = permit;
                    if cancel.is_canceled() {
                        return Ok(None);
                    }
                    match Syntax::new(text.slice(..), language, &loader) {
                        Ok(syntax) => Ok(Some(syntax)),
                        // Missing/invalid configuration has already been reported by the loader.
                        Err(syntax::HighlighterError::NoRootConfig) => Ok(None),
                        Err(err) => Err(anyhow::anyhow!("{err}")),
                    }
                })
                .await?
            },
            &self.cancel,
        )
        .await
    }

    /// Publish on the editor thread. Superseded requests are ignored, and edits
    /// made during compilation start a fresh snapshot using the cached queries.
    /// Returns whether initialization finished and dependent features can refresh.
    pub fn complete(self, doc: &mut Document, result: anyhow::Result<Option<Syntax>>) -> bool {
        if self.cancel.is_canceled() {
            return false;
        }
        if self.version != doc.version {
            doc.initialize_syntax(self.loader);
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
    /// Capture the current text if initialization has not already been scheduled.
    pub fn syntax_request(&mut self) -> Option<SyntaxRequest> {
        let pending = self.pending_syntax.as_mut()?;
        if pending.controller.is_running() {
            return None;
        }
        Some(SyntaxRequest {
            text: self.text.clone(),
            version: self.version,
            language: self.language.as_ref()?.language(),
            loader: pending.loader.clone(),
            cancel: pending.controller.restart(),
        })
    }

    pub(crate) fn initialize_syntax(&mut self, loader: Arc<syntax::Loader>) {
        self.syntax = None;
        self.pending_syntax = self.language.as_ref().map(|_| PendingSyntax {
            loader,
            controller: TaskController::new(),
        });
        if self.pending_syntax.is_some() {
            event::dispatch(crate::events::DocumentSyntaxRequested { doc: self });
        }
    }

    /// Whether the document is waiting for its initial syntax tree.
    pub fn is_syntax_pending(&self) -> bool {
        self.pending_syntax.is_some()
    }

    /// Finish initialization on demand for commands that require a syntax tree.
    pub(crate) fn finish_syntax_initialization(&mut self) -> bool {
        let Some(mut pending) = self.pending_syntax.take() else {
            return false;
        };
        pending.controller.cancel();
        self.set_language(self.language.clone(), &pending.loader);
        pending.loader.refresh_highlights();
        true
    }
}

#[cfg(test)]
mod tests {
    use arc_swap::ArcSwap;
    use editor_core::{Selection, Transaction};

    use super::*;
    use crate::{
        editor::Config,
        events::{DocumentDidChange, SelectionDidChange},
        ViewId,
    };

    fn loader() -> Arc<syntax::Loader> {
        event::runtime_local! {
            static REGISTER: std::sync::Once = std::sync::Once::new();
        }
        REGISTER.call_once(event::register_event::<crate::events::DocumentSyntaxRequested>);
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
            )
            .unwrap(),
        )
    }

    fn document(loader: &Arc<syntax::Loader>) -> Document {
        Document::from(
            Rope::from_str("{}"),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            Arc::new(ArcSwap::new(loader.clone())),
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
        assert!(doc.apply(&transaction, view));
        assert!(doc.syntax().is_none());

        let result = request.compute().await.unwrap();
        assert!(!request.complete(&mut doc, result));
        assert!(doc.syntax().is_none());
        finish(&mut doc).await;
        let root = doc.syntax().unwrap().tree().root_node();
        assert_eq!(root.byte_range(), 0..doc.text().len_bytes() as u32);
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
        doc.syn_loader.store(new_loader.clone());
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
