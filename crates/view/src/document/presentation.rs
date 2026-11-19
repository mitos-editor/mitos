//! Isolated rendering state for proposed edits. No live services are copied.

use super::Document;
use crate::ViewId;
use editor_core::{ChangeSet, Rope, Selection, Transaction};
use std::{collections::HashMap, sync::OnceLock};
use stdx::rope::RopeExt as _;

/// Proposed text and rendering values, without request controllers, protocol
/// attachments, or editor services. The buffer retains snapshot identity for
/// renderer caches, but records no edit history.
pub(crate) struct Presentation {
    buffer: buffer::Buffer,
    syntax: Option<language::Syntax>,
    views: HashMap<ViewId, ViewPresentation>,
    diagnostics: diagnostics::DiagnosticStore,
    colors: Option<super::DocumentColorSwatches>,
    links: Vec<super::DocumentLink>,
    lenses: Vec<super::CodeLens>,
    snippet: Option<super::SnippetSession>,
    conflicts: OnceLock<crate::features::conflicts::ConflictPresentation>,
}

struct ViewPresentation {
    selection: Selection,
    folds: super::folding::Folds,
    position: crate::view::ViewPosition,
    jump_labels: Vec<text_layout::text_annotations::Overlay>,
    inlay_hints: Option<super::DocumentInlayHints>,
    highlights: Option<Vec<std::ops::Range<usize>>>,
    code_action_hints: bool,
    breadcrumbs: Option<Vec<super::Crumb>>,
}

/// A rendering borrow combines source metadata with either live or proposed text.
#[derive(Clone, Copy)]
pub struct PresentedDocument<'a> {
    source: &'a Document,
    presentation: Option<&'a Presentation>,
}

impl<'a> From<&'a Document> for PresentedDocument<'a> {
    fn from(source: &'a Document) -> Self {
        Self {
            source,
            presentation: None,
        }
    }
}
impl<'a> From<&'a mut Document> for PresentedDocument<'a> {
    fn from(source: &'a mut Document) -> Self {
        Self::from(&*source)
    }
}
impl<'a> PresentedDocument<'a> {
    pub(crate) fn proposed(source: &'a Document, presentation: &'a Presentation) -> Self {
        Self {
            source,
            presentation: Some(presentation),
        }
    }
    pub fn source(self) -> &'a Document {
        self.source
    }
    pub fn is_live(self) -> bool {
        self.presentation.is_none()
    }
    pub fn text(self) -> &'a Rope {
        self.presentation
            .map_or_else(|| self.source.text(), |p| p.buffer.text())
    }
    pub fn snapshot(self) -> buffer::Snapshot {
        self.presentation
            .map_or_else(|| self.source.snapshot(), |p| p.buffer.snapshot())
    }
    pub fn is_snapshot_current(self, snapshot: &buffer::Snapshot) -> bool {
        self.presentation.map_or_else(
            || self.source.is_snapshot_current(snapshot),
            |p| snapshot.is_current(&p.buffer),
        )
    }
    pub fn syntax(self) -> Option<&'a language::Syntax> {
        self.presentation
            .map_or_else(|| self.source.syntax(), |p| p.syntax.as_ref())
    }
    pub fn syntax_loader(self) -> &'a language::syntax::Loader {
        self.source.syntax_loader()
    }
    pub fn selection(self, view: ViewId) -> &'a Selection {
        self.presentation.map_or_else(
            || self.source.selection(view),
            |p| &p.views[&view].selection,
        )
    }
    pub fn cursor_line(self, view: ViewId) -> usize {
        self.text()
            .char_to_line_idx(self.selection(view).primary().cursor(self.text().slice(..)))
    }
    pub fn view_offset(self, view: ViewId) -> crate::view::ViewPosition {
        self.presentation.map_or_else(
            || self.source.view_offset(view),
            |p| p.views[&view].position,
        )
    }
    pub fn folded_ranges(self, view: ViewId) -> &'a [text_layout::text_annotations::Conceal] {
        self.presentation.map_or_else(
            || self.source.folded_ranges(view),
            |p| p.views.get(&view).map_or(&[], |v| v.folds.visible()),
        )
    }
    pub(crate) fn jump_labels(
        self,
        view: ViewId,
    ) -> Option<&'a [text_layout::text_annotations::Overlay]> {
        self.presentation.map_or_else(
            || self.source.jump_labels(view),
            |p| {
                p.views
                    .get(&view)
                    .and_then(|v| (!v.jump_labels.is_empty()).then_some(v.jump_labels.as_slice()))
            },
        )
    }
    pub fn inlay_hints(self, view: ViewId) -> Option<&'a super::DocumentInlayHints> {
        self.presentation.map_or_else(
            || self.source.inlay_hints(view),
            |p| p.views.get(&view)?.inlay_hints.as_ref(),
        )
    }
    pub fn document_highlights(self, view: ViewId) -> Option<&'a [std::ops::Range<usize>]> {
        self.presentation.map_or_else(
            || self.source.document_highlights(view),
            |p| p.views.get(&view)?.highlights.as_deref(),
        )
    }
    pub fn code_action_hints(self, view: ViewId) -> bool {
        self.presentation.map_or_else(
            || self.source.code_action_hints(view),
            |p| p.views.get(&view).is_some_and(|v| v.code_action_hints),
        )
    }
    pub fn breadcrumbs(self, view: ViewId) -> Option<&'a [super::Crumb]> {
        self.presentation.map_or_else(
            || self.source.breadcrumbs(view),
            |p| p.views.get(&view)?.breadcrumbs.as_deref(),
        )
    }
    pub fn diagnostics(self) -> &'a [diagnostics::Diagnostic] {
        self.presentation
            .map_or_else(|| self.source.diagnostics(), |p| p.diagnostics.as_slice())
    }
    pub fn color_swatches(self) -> Option<&'a super::DocumentColorSwatches> {
        self.presentation
            .map_or_else(|| self.source.color_swatches(), |p| p.colors.as_ref())
    }
    pub fn document_links(self) -> &'a [super::DocumentLink] {
        self.presentation
            .map_or_else(|| self.source.document_links(), |p| &p.links)
    }
    pub fn code_lenses(self) -> &'a [super::CodeLens] {
        self.presentation
            .map_or_else(|| self.source.code_lenses(), |p| &p.lenses)
    }
    pub fn active_snippet(self) -> Option<&'a snippets::ActiveSnippet> {
        self.presentation.map_or_else(
            || self.source.active_snippet(),
            |p| p.snippet.as_ref().map(|s| &s.active),
        )
    }
    pub fn semantic_tokens(self) -> &'a [crate::features::semantic_tokens::SemanticToken] {
        if self.is_live() {
            self.source.semantic_tokens()
        } else {
            &[]
        }
    }
    pub fn semantic_tokens_revision(self) -> u64 {
        if self.is_live() {
            self.source.semantic_tokens_revision()
        } else {
            0
        }
    }
    pub fn inline_completion(self, view: ViewId) -> Option<&'a super::InlineCompletion> {
        self.is_live()
            .then(|| self.source.inline_completion(view))
            .flatten()
    }
    pub fn search_highlight(
        self,
        view: ViewId,
    ) -> Option<&'a crate::features::buffer_search::Query> {
        self.is_live()
            .then(|| self.source.search_highlight(view))
            .flatten()
    }
    pub fn supports_language_server(self, server: lsp_client::LanguageServerId) -> bool {
        self.is_live() && self.source.supports_language_server(server)
    }
    pub fn conflict_presentation(self) -> &'a crate::features::conflicts::ConflictPresentation {
        self.presentation.map_or_else(
            || self.source.conflict_presentation(),
            |p| {
                p.conflicts.get_or_init(|| {
                    crate::features::conflicts::ConflictPresentation::new(p.buffer.text())
                })
            },
        )
    }
}

pub(super) fn map_view(
    selection: &mut Selection,
    folds: &mut super::folding::Folds,
    position: &mut crate::view::ViewPosition,
    changes: &ChangeSet,
    text: &Rope,
) {
    folds.apply_changes(changes);
    *selection = selection
        .clone()
        .map(changes)
        .ensure_invariants(text.slice(..));
    position.anchor = changes.map_pos(position.anchor, editor_core::Assoc::Before);
    folds.reveal(selection, text.slice(..));
}

pub(super) fn map_syntax(
    syntax: &mut Option<language::Syntax>,
    pending: bool,
    change: &buffer::AppliedChange,
    text: &Rope,
    loader: &language::syntax::Loader,
) {
    let result = match syntax {
        Some(syntax) if pending => syntax.update_compiled(
            change.before().slice(..),
            text.slice(..),
            change.changes(),
            loader,
        ),
        Some(syntax) => syntax.update(
            change.before().slice(..),
            text.slice(..),
            change.changes(),
            loader,
        ),
        None => Ok(()),
    };
    if let Err(error) = result {
        log::error!("TS parser failed, disabling TS for the current buffer: {error}");
        *syntax = None;
    }
}

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

    pub(crate) fn presentation(&self, transaction: &Transaction, origin: ViewId) -> Presentation {
        let mut presentation = Presentation {
            buffer: buffer::Buffer::new(self.text().clone()),
            syntax: self.syntax.clone(),
            views: self
                .view_states
                .iter()
                .map(|(&id, state)| {
                    (
                        id,
                        ViewPresentation {
                            selection: state.selection.clone(),
                            folds: state.folds.clone(),
                            position: state.position,
                            jump_labels: state.jump_labels.clone(),
                            inlay_hints: self.inlay_hints(id).cloned(),
                            highlights: self.document_highlights(id).map(<[_]>::to_vec),
                            code_action_hints: self.code_action_hints(id),
                            breadcrumbs: state.breadcrumbs.clone(),
                        },
                    )
                })
                .collect(),
            diagnostics: self.diagnostics.clone(),
            colors: self.color_swatches().cloned(),
            links: self.document_links().to_vec(),
            lenses: self.code_lenses().to_vec(),
            snippet: self.snippet.clone(),
            conflicts: Default::default(),
        };
        let change = presentation
            .buffer
            .apply_without_history(transaction.changes())
            .expect("validated preview");
        for state in presentation.views.values_mut() {
            map_view(
                &mut state.selection,
                &mut state.folds,
                &mut state.position,
                change.changes(),
                presentation.buffer.text(),
            );
            if let Some(hints) = &mut state.inlay_hints {
                hints.apply_changes(change.changes());
            }
            if let Some(highlights) = &mut state.highlights {
                crate::features::document_highlight::DocumentHighlightsState::map(
                    highlights,
                    change.changes(),
                    presentation.buffer.text().len_chars(),
                );
            }
        }
        map_syntax(
            &mut presentation.syntax,
            false,
            &change,
            presentation.buffer.text(),
            &self.syntax_loader,
        );
        presentation
            .diagnostics
            .apply_changes(change.changes(), presentation.buffer.text());
        if let Some(colors) = &mut presentation.colors {
            colors.apply_changes(change.changes());
        }
        super::DocumentLink::apply_changes(&mut presentation.links, change.changes());
        super::CodeLens::apply_changes(&mut presentation.lenses, change.changes());
        super::snippet::map(&mut presentation.snippet, change.changes());
        if let Some(selection) = transaction.selection() {
            let state = presentation.views.get_mut(&origin).expect("preview origin");
            let selection = selection
                .clone()
                .ensure_invariants(presentation.buffer.text().slice(..));
            let changed = state.selection != selection;
            state.selection = selection;
            state
                .folds
                .reveal(&state.selection, presentation.buffer.text().slice(..));
            super::snippet::validate(
                &mut presentation.snippet,
                origin,
                state.selection.clone(),
                presentation.buffer.text(),
                changed,
            );
        }
        presentation
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
        let events = Arc::new(AtomicUsize::new(0));
        let observed = events.clone();
        let mut doc = Document::from(
            Rope::from("界abc\n"),
            None,
            Arc::new(arc_swap::ArcSwap::from_pointee(
                editor_config::Config::default(),
            )),
            Arc::new(language::syntax::Loader::default()),
        );
        event::register_hook!(doc.events() => move |_event: &mut DocumentDidChange<'_>| {
            observed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        let view = ViewId::default();
        doc.ensure_view_init(view);
        doc.set_document_highlights(view, std::iter::once(1..4).collect());
        doc.set_code_action_hints(view);
        let mut inlay_hints =
            super::super::DocumentInlayHints::empty_with_id(super::super::DocumentInlayHintsId {
                first_line: 0,
                last_line: 1,
            });
        inlay_hints
            .type_inlay_hints
            .push(text_layout::text_annotations::InlineAnnotation::new(
                1, "hint",
            ));
        doc.set_inlay_hints(view, inlay_hints);
        let highlights = doc.document_highlight_controller(view).restart();
        let hints = doc.code_action_controller(view).restart();
        let request = doc.rename.restart();
        let state = doc.view_state_mut(view);
        let view_requests = [
            state.hover.get_or_insert_default().restart(),
            state.navigation.get_or_insert_default().restart(),
            state.code_actions.get_or_insert_default().restart(),
            state.shell_filters.get_or_insert_default().restart(),
        ];
        let before = doc.snapshot();
        let selection = doc.selection(view).clone();
        let transaction =
            Transaction::change(doc.text(), [(0, 1, Some("é\n\t".into()))].into_iter())
                .with_selection(Selection::point(3));
        let state = doc.presentation(&transaction, view);
        let presentation = PresentedDocument::proposed(&doc, &state);
        assert_eq!(presentation.text(), "é\n\tabc\n");
        assert_eq!(
            presentation.document_highlights(view),
            Some(std::slice::from_ref(&(3..6)))
        );
        assert_eq!(presentation.selection(view), &Selection::single(3, 4));
        assert!(presentation.code_action_hints(view));
        assert_eq!(
            presentation.inlay_hints(view).unwrap().type_inlay_hints[0].char_idx,
            3
        );
        assert_eq!(doc.text(), before.text());
        assert_eq!(doc.snapshot().version(), before.version());
        assert_eq!(doc.selection(view), &selection);
        assert_eq!(events.load(Ordering::Relaxed), 0);
        drop(state);
        assert!(!request.is_canceled());
        assert!(!highlights.is_canceled());
        assert!(!hints.is_canceled());
        assert!(view_requests.iter().all(|request| !request.is_canceled()));
        crate::editing::apply(&mut doc, &transaction, view).unwrap();
        assert_eq!(events.load(Ordering::Relaxed), 1);
    }
}
