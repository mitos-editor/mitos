//! Apply server-provided document locations without frontend or transport state.

use crate::{align_view, editor::Action, Align, DocumentId, Editor};
use anyhow::{Context, Result};
use editor_core::{Selection, Uri};
use lsp_client::{lsp, util::lsp_range_to_range, OffsetEncoding};

impl Editor {
    /// Open a local document and position a view displaying it. Background loads
    /// preserve focus and defer hidden destinations until their next display.
    pub fn open_document_location(
        &mut self,
        uri: &Uri,
        selection: Option<lsp::Range>,
        encoding: OffsetEncoding,
        action: Action,
    ) -> Result<DocumentId> {
        let path = uri
            .as_path()
            .context("document location requires a local file URI")?;
        // A server can reposition the cursor in the focused document while typing.
        // Reopening that document would switch the editor to normal mode.
        let focused = self
            .tree
            .try_get(self.tree.focus())
            .and_then(|view| self.document(view.document_id()));
        let (document, in_place) = match focused {
            Some(doc)
                if matches!(action, Action::Replace)
                    && doc
                        .path()
                        .is_some_and(|p| p == stdx::path::canonicalize(path)) =>
            {
                (doc.id(), true)
            }
            _ => (self.open(path, action)?, false),
        };
        // Prefer the focused destination, otherwise an existing split. A hidden
        // buffer has no displayed selection to position.
        let target = self
            .tree
            .views()
            .filter(|(view, _)| view.document_id() == document)
            .max_by_key(|(_, focused)| *focused)
            .map(|(view, _)| view.id);
        if let Some(range) = selection {
            let doc = self.documents.get_mut(&document).unwrap();
            if let Some(range) = lsp_range_to_range(doc.text(), range, encoding) {
                // Keep the cursor at the start of the destination symbol.
                let selection = Selection::single(range.head, range.anchor);
                if let Some(view) = target {
                    doc.set_selection(view, selection);
                    if in_place {
                        // Keep the user's scroll position. Only scroll if the cursor would leave the view.
                        self.ensure_cursor_in_view(view);
                    } else {
                        align_view(doc, self.tree.get_mut(view), Align::Center);
                    }
                } else {
                    doc.pending_navigation = Some(selection);
                }
            } else {
                log::warn!("LSP document position out of bounds: {range:?}");
            }
        }
        Ok(document)
    }
}
