//! The common text mutation path, including mandatory document invariants.

use super::Document;
use crate::editing::EditError;
use crate::{
    events::{DocumentDidChange, SelectionDidChange},
    ViewId,
};
use buffer::AppliedChange;
use editor_core::{ChangeSet, Selection, Transaction};

impl Document {
    /// Apply text changes without requiring a view. Call [`Self::commit_changes`]
    /// to end the undo group, or use [`crate::Editor::apply_document_changes`] for an
    /// independent, version-checked operation on an editor-owned document.
    pub fn apply(&mut self, changes: &ChangeSet) -> Result<(), EditError> {
        let transaction = Transaction::from(changes.clone());
        self.apply_inner(&transaction, None)
    }

    pub(crate) fn validate_changes(&self, changes: &ChangeSet) -> Result<(), EditError> {
        self.buffer.validate_changes(changes)?;
        if !changes.is_empty() {
            self.validate_editable()?;
        }
        Ok(())
    }

    pub(crate) fn validate_editable(&self) -> Result<(), EditError> {
        if self.is_virtual() {
            return Err(EditError::ReadOnlyDocument);
        }
        if self.is_binary() {
            return Err(EditError::BinaryDocument);
        }
        Ok(())
    }

    pub(crate) fn apply_inner(
        &mut self,
        transaction: &Transaction,
        origin: Option<ViewId>,
    ) -> Result<(), EditError> {
        self.validate_changes(transaction.changes())?;
        let change = self.buffer.apply(transaction.changes())?;
        self.publish_change(&change, transaction.selection(), origin);
        Ok(())
    }

    pub(super) fn map_presentation(
        &mut self,
        change: &AppliedChange,
        selection: Option<&Selection>,
        view_id: Option<ViewId>,
    ) {
        use editor_core::Assoc;

        let old_doc = change.before();
        let changes = change.changes();
        if !changes.is_empty() {
            self.invalidate_search_counts();
        }
        for state in self.view_states.values_mut() {
            state.expansion_history.clear();
            let selection = &mut state.selection;
            *selection = selection
                .clone()
                // Map through changes
                .map(changes)
                // Ensure all selections across all views still adhere to invariants.
                .ensure_invariants(self.buffer.text().slice(..));
            state.position.anchor = changes.map_pos(state.position.anchor, Assoc::Before);
        }

        // update tree-sitter syntax tree
        if let Some(syntax) = &mut self.syntax
            && let Err(err) = syntax.update(
                old_doc.slice(..),
                self.buffer.text().slice(..),
                changes,
                &self.syntax_loader,
            )
        {
            log::error!("TS parser failed, disabling TS for the current buffer: {err}");
            self.syntax = None;
        }

        self.diagnostics.apply_changes(changes, self.buffer.text());
        self.document_colors.apply_changes(changes);
        self.document_links.apply_changes(changes);
        self.code_lenses.apply_changes(changes);
        self.inline_completions.clear();
        self.map_snippet(changes);

        self.inlay_hints.apply_changes(changes);
        self.hover.clear();
        self.navigation.clear();

        self.document_highlights
            .apply_changes(changes, self.buffer.text().len_chars());

        // Install an explicit transaction selection before observers inspect the new state.
        if let (Some(selection), Some(view_id)) = (selection, view_id) {
            self.update_selection(view_id, selection.clone());
        }
    }

    /// Map mandatory document state, queue protocol synchronization, then notify observers.
    pub(super) fn publish_change(
        &mut self,
        change: &AppliedChange,
        selection: Option<&Selection>,
        view_id: Option<ViewId>,
    ) {
        let old_doc = change.before();
        let changes = change.changes();
        if changes.is_empty() {
            if let (Some(selection), Some(view_id)) = (selection, view_id) {
                self.update_selection(view_id, selection.clone());
                let errors = event::dispatch(SelectionDidChange {
                    doc: self,
                    view: view_id,
                });
                self.report_hook_errors(errors);
            }
            return;
        }

        self.modified_since_accessed = true;
        self.navigation
            .map_pending_location(changes, self.buffer.text().slice(..));
        self.map_presentation(change, selection, view_id);
        // start computing the diff in parallel
        if let Some(diff_handle) = &self.diff_handle {
            diff_handle.update_document(self.buffer.text().clone(), false);
        }

        // Map cached anchors as part of the mutation, before protocol synchronization or observers.
        if let Some(source) = &self.breakpoint_source {
            crate::debugger::map_breakpoint_positions(source, changes, old_doc, self.buffer.text());
        }
        crate::handlers::lsp::synchronize_document(self, old_doc, changes);

        // Observers see mapped caches and the final selection, and may now schedule work.
        let errors = event::dispatch(DocumentDidChange {
            doc: self,
            view: view_id,
            old_text: old_doc,
            changes,
        });
        self.report_hook_errors(errors);
        if let (Some(_), Some(view_id)) = (selection, view_id) {
            let errors = event::dispatch(SelectionDidChange {
                doc: self,
                view: view_id,
            });
            self.report_hook_errors(errors);
        }
    }
}
