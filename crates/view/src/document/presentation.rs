//! Isolated rendering state for proposed edits. No live services are copied.

use super::Document;
use crate::ViewId;
use editor_core::Transaction;

impl Document {
    /// Refresh an unregistered file preview without dropping compatible syntax.
    pub fn refresh_preview(&mut self, mut refreshed: Self) {
        let same_language = match (&self.language, &refreshed.language) {
            (Some(previous), Some(current)) => std::sync::Arc::ptr_eq(previous, current),
            (None, None) => true,
            _ => false,
        };
        if same_language
            && std::sync::Arc::ptr_eq(&self.syntax_loader, &refreshed.syntax_loader)
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

    pub(crate) fn presentation(&self, transaction: &Transaction, origin: ViewId) -> Self {
        let mut doc = Self::from(
            self.text().clone(),
            None,
            self.config.clone(),
            self.syntax_loader.clone(),
        );
        doc.id = self.id;
        doc.file = self.file.clone();
        doc.editor_config = self.editor_config.clone();
        doc.diff_handle = self.diff_handle.clone();
        doc.version_control_head = self.version_control_head.clone();
        doc.blame = self.blame.presentation();
        doc.indent_style = self.indent_style;
        doc.line_ending = self.line_ending;
        doc.language = self.language.clone();
        doc.syntax = self.syntax.clone();
        doc.view_states = self.view_states.clone();
        doc.snippet = self.snippet.clone();
        doc.diagnostics = self.diagnostics.clone();
        doc.document_colors = self.document_colors.presentation();
        doc.document_links = self.document_links.presentation();
        doc.code_lenses = self.code_lenses.presentation();
        doc.document_highlights = self.document_highlights.presentation();
        doc.document_symbols = self.document_symbols.presentation();
        doc.code_action_hints = self.code_action_hints.presentation();
        doc.inlay_hints = self.inlay_hints.presentation();
        let change = doc
            .buffer
            .apply(transaction.changes())
            .expect("validated preview");
        doc.map_presentation(&change, transaction.selection(), Some(origin));
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::DocumentDidChange;
    use editor_core::{Rope, Selection};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn presentation_maps_annotations_without_publishing_or_canceling_live_work() {
        crate::events::register();
        let events = Arc::new(AtomicUsize::new(0));
        let observed = events.clone();
        event::register_hook!(move |_event: &mut DocumentDidChange<'_>| {
            observed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        let mut doc = Document::from(
            Rope::from("界abc\n"),
            None,
            Arc::new(arc_swap::ArcSwap::from_pointee(
                editor_config::Config::default(),
            )),
            Arc::new(language::syntax::Loader::default()),
        );
        let view = ViewId::default();
        doc.ensure_view_init(view);
        doc.set_document_highlights(view, std::iter::once(1..4).collect());
        let request = doc.rename.restart();
        let before = doc.snapshot();
        let selection = doc.selection(view).clone();
        let transaction =
            Transaction::change(doc.text(), [(0, 1, Some("é\n\t".into()))].into_iter())
                .with_selection(Selection::point(3));
        let presentation = doc.presentation(&transaction, view);
        assert_eq!(presentation.text(), "é\n\tabc\n");
        assert_eq!(
            presentation.document_highlights(view),
            Some(std::slice::from_ref(&(3..6)))
        );
        assert_eq!(presentation.selection(view), &Selection::single(3, 4));
        assert_eq!(doc.text(), before.text());
        assert_eq!(doc.snapshot().version(), before.version());
        assert_eq!(doc.selection(view), &selection);
        assert_eq!(events.load(Ordering::Relaxed), 0);
        drop(presentation);
        assert!(!request.is_canceled());
        crate::editing::apply(&mut doc, &transaction, view).unwrap();
        assert_eq!(events.load(Ordering::Relaxed), 1);
    }
}
