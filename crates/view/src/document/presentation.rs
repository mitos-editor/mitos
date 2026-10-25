//! Isolated rendering state for proposed edits. No live services are copied.

use super::Document;
use crate::ViewId;
use editor_core::Transaction;

impl Document {
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
        doc.document_symbols = self.document_symbols.presentation();
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
        let requests = [
            doc.rename.restart(),
            doc.document_highlight_controller(view).restart(),
            doc.code_action_controller(view).restart(),
            doc.view_state_mut(view).hover.restart(),
            doc.view_state_mut(view).code_actions.restart(),
            doc.view_state_mut(view).shell_filter.restart(),
        ];
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
        assert!(requests.iter().all(|request| !request.is_canceled()));
        crate::editing::apply(&mut doc, &transaction, view).unwrap();
        assert_eq!(events.load(Ordering::Relaxed), 1);
    }
}
