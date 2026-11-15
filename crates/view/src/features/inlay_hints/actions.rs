//! Operations on a selected hint, retaining its document and server ownership.

use anyhow::{ensure, Context as _, Result};
use lsp_client::lsp;

use super::ResolvedInlayHint;
use crate::{editor::Action, features::navigation::Location, Editor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlayHintAction {
    LabelLocation(usize),
    LabelCommand(usize),
    ApplyTextEdits,
}

impl ResolvedInlayHint {
    /// Available operations in label-part order, followed by accepting the hint.
    pub fn actions(&self) -> Vec<InlayHintAction> {
        let mut actions = Vec::new();
        if let lsp::InlayHintLabel::LabelParts(parts) = &self.hint().label {
            for (index, part) in parts.iter().enumerate() {
                if part.location.is_some() {
                    actions.push(InlayHintAction::LabelLocation(index));
                }
                if part.command.is_some() {
                    actions.push(InlayHintAction::LabelCommand(index));
                }
            }
        }
        if self
            .hint()
            .text_edits
            .as_ref()
            .is_some_and(|edits| !edits.is_empty())
        {
            actions.push(InlayHintAction::ApplyTextEdits);
        }
        actions
    }

    /// Execute only against the unchanged interaction that supplied the hint.
    pub fn execute(
        &self,
        editor: &mut Editor,
        action: InlayHintAction,
        open: Action,
    ) -> Result<()> {
        ensure!(self.is_current(editor), "Inlay hint is no longer current");
        ensure!(
            editor.tree.focus() == self.view_id()
                && editor
                    .tree
                    .try_get(self.view_id())
                    .is_some_and(|view| view.document_id() == self.document_id()),
            "Inlay hint's view is no longer focused"
        );
        match action {
            InlayHintAction::LabelLocation(index) => {
                let part = self.label_part(index)?;
                let location = Location::from_lsp(
                    part.location
                        .clone()
                        .context("Inlay label part has no location")?,
                    self.offset_encoding(),
                )
                .context("Inlay label part has an unsupported location")?;
                ensure!(
                    location.range.start <= location.range.end,
                    "Inlay label part has a reversed location range"
                );
                let (view, doc) = crate::current!(editor);
                doc.commit_changes();
                view.push_jump(doc, (doc.id(), doc.selection(view.id).clone()));
                editor.open_document_location(
                    &location.uri,
                    Some(location.range),
                    location.offset_encoding,
                    open,
                )?;
                let padding = editor.config().scroll.padding;
                let (view, doc) = crate::current!(editor);
                view.ensure_cursor_in_view(doc, padding);
            }
            InlayHintAction::LabelCommand(index) => {
                let command = self
                    .label_part(index)?
                    .command
                    .clone()
                    .context("Inlay label part has no command")?;
                editor.execute_lsp_command(command, self.language_server_id());
            }
            InlayHintAction::ApplyTextEdits => {
                let edits = self
                    .hint()
                    .text_edits
                    .as_ref()
                    .filter(|edits| !edits.is_empty())
                    .context("Inlay hint has no text edits")?;
                let padding = editor.config().scroll.padding;
                let (view, doc) = crate::current!(editor);
                // Validate the complete array and document permissions before
                // changing text or committing the user's pending undo group.
                let transaction = crate::language_servers::edits::generate_transaction_from_edits(
                    doc.text(),
                    edits.clone(),
                    self.offset_encoding(),
                )?;
                doc.validate_changes(transaction.changes())?;
                if transaction.changes().is_empty() {
                    return Ok(());
                }
                doc.commit_changes();
                doc.linked_editing.clear();
                crate::editing::apply(doc, &transaction, view.id)?;
                doc.commit_changes();
                view.sync_changes(doc);
                view.ensure_cursor_in_view(doc, padding);
                crate::features::linked_editing::refresh(editor);
            }
        }
        Ok(())
    }

    fn label_part(&self, index: usize) -> Result<&lsp::InlayHintLabelPart> {
        let lsp::InlayHintLabel::LabelParts(parts) = &self.hint().label else {
            anyhow::bail!("Inlay hint has no structured label parts")
        };
        parts
            .get(index)
            .context("Inlay hint label part is unavailable")
    }
}
