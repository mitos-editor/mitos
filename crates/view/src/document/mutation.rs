//! The common text mutation path, including mandatory document invariants.

use super::Document;
use crate::editing::EditError;
use crate::{
    events::{DocumentDidChange, SelectionDidChange},
    ViewId,
};
use editor_core::{ChangeSet, Transaction};
use std::mem;

impl Document {
    /// Apply text changes without requiring a view. Call [`Self::commit_changes`]
    /// to end the undo group, or use [`crate::Editor::apply_document_changes`] for an
    /// independent, version-checked operation on an editor-owned document.
    pub fn apply(&mut self, changes: &ChangeSet) -> Result<(), EditError> {
        let transaction = Transaction::from(changes.clone());
        self.apply_inner(&transaction, None, true)
    }

    pub(crate) fn validate_changes(&self, changes: &ChangeSet) -> Result<(), EditError> {
        if changes.len_before() != self.text.len_chars() {
            return Err(EditError::InvalidLength {
                expected: self.text.len_chars(),
                actual: changes.len_before(),
            });
        }
        if self.binary && !changes.is_empty() {
            return Err(EditError::BinaryDocument);
        }
        Ok(())
    }

    pub(crate) fn apply_inner(
        &mut self,
        transaction: &Transaction,
        origin: Option<ViewId>,
        emit_lsp_notification: bool,
    ) -> Result<(), EditError> {
        let before = self.text.clone();
        self.apply_impl(transaction, origin, emit_lsp_notification)?;
        if !transaction.changes().is_empty() {
            self.pending_original_text.get_or_insert(before);
            self.changes = mem::take(&mut self.changes).compose(transaction.changes().clone());
        }
        Ok(())
    }

    /// Apply a [`Transaction`] to the [`Document`] to change its text.
    pub(crate) fn apply_impl(
        &mut self,
        transaction: &Transaction,
        view_id: Option<ViewId>,
        emit_lsp_notification: bool,
    ) -> Result<(), EditError> {
        use editor_core::Assoc;

        self.validate_changes(transaction.changes())?;
        let old_doc = self.text().clone();
        let changes = transaction.changes();
        if !changes.apply(&mut self.text) {
            return Err(EditError::InvalidLength {
                expected: self.text.len_chars(),
                actual: changes.len_before(),
            });
        }

        if changes.is_empty() {
            if let (Some(selection), Some(view_id)) = (transaction.selection(), view_id) {
                self.update_selection(view_id, selection.clone());
                let errors = event::dispatch(SelectionDidChange {
                    doc: self,
                    view: view_id,
                });
                self.report_hook_errors(errors);
            }
            return Ok(());
        }

        self.modified_since_accessed = true;
        self.version += 1;

        for selection in self.selections.values_mut() {
            *selection = selection
                .clone()
                // Map through changes
                .map(transaction.changes())
                // Ensure all selections across all views still adhere to invariants.
                .ensure_invariants(self.text.slice(..));
        }

        for view_data in self.view_data.values_mut() {
            view_data.view_position.anchor = transaction
                .changes()
                .map_pos(view_data.view_position.anchor, Assoc::Before);
        }

        // generate revert to savepoint
        if !self.savepoints.is_empty() {
            let revert = transaction.invert(&old_doc);
            self.savepoints
                .retain_mut(|save_point| match save_point.upgrade() {
                    Some(savepoint) => {
                        let mut revert_to_savepoint = savepoint.revert.lock();
                        *revert_to_savepoint =
                            revert.clone().compose(mem::take(&mut revert_to_savepoint));
                        true
                    }
                    None => false,
                })
        }

        // update tree-sitter syntax tree
        if let Some(syntax) = &mut self.syntax {
            let loader = self.syn_loader.load();
            if let Err(err) = syntax.update(
                old_doc.slice(..),
                self.text.slice(..),
                transaction.changes(),
                &loader,
            ) {
                log::error!("TS parser failed, disabling TS for the current buffer: {err}");
                self.syntax = None;
            }
        }

        // start computing the diff in parallel
        if let Some(diff_handle) = &self.diff_handle {
            diff_handle.update_document(self.text.clone(), false);
        }

        // Map cached anchors as part of the mutation, before protocol synchronization or observers.
        self.map_diagnostics(changes);
        self.document_colors.apply_changes(changes);
        self.document_links.apply_changes(changes);
        self.code_lenses
            .apply_changes(changes, &old_doc, &self.text, !emit_lsp_notification);
        self.inline_completions.clear();
        self.map_snippet(changes);

        self.inlay_hints.apply_changes(changes);
        self.hover.clear();
        self.navigation.clear();

        self.document_highlights
            .apply_changes(changes, self.text.len_chars());

        // Install an explicit transaction selection before observers inspect the new state.
        if let (Some(selection), Some(view_id)) = (transaction.selection(), view_id) {
            self.update_selection(view_id, selection.clone());
        }

        // Temporary completion previews deliberately remain local to the editor.
        if emit_lsp_notification {
            crate::handlers::lsp::synchronize_document(self, &old_doc, changes);
        }

        // Observers see mapped caches and the final selection, and may now schedule work.
        let errors = event::dispatch(DocumentDidChange {
            doc: self,
            view: view_id,
            old_text: &old_doc,
            changes,
            ghost_transaction: !emit_lsp_notification,
        });
        self.report_hook_errors(errors);
        if let (Some(_), Some(view_id)) = (transaction.selection(), view_id) {
            let errors = event::dispatch(SelectionDidChange {
                doc: self,
                view: view_id,
            });
            self.report_hook_errors(errors);
        }

        Ok(())
    }
}
