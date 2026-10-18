//! Resolve missing completion edits after inserting the selected text.

use std::sync::{Arc, Weak};

use editor_core::{Assoc, ChangeSet, Rope, Transaction};
use event::{TaskController, TaskHandle};
use lsp_client::{lsp, util::lsp_range_to_range};

use super::LspCompletionItem;
use crate::{document::SavePoint, DocumentId, Editor};

/// The provider snapshot and savepoint captured before inserting the selected text.
pub(super) struct AcceptedCompletion {
    pub document: DocumentId,
    pub item: LspCompletionItem,
    pub before: buffer::Snapshot,
    pub savepoint: Arc<SavePoint>,
}

/// Document-owned resolution requests that survive completion-menu replacement.
#[derive(Default)]
pub(crate) struct AcceptedCompletions {
    requests: Vec<TaskController>,
}

impl AcceptedCompletions {
    fn start(&mut self) -> TaskHandle {
        self.requests.retain(TaskController::is_running);
        let mut controller = TaskController::new();
        let handle = controller.restart();
        self.requests.push(controller);
        handle
    }

    pub(crate) fn cancel(&mut self) {
        self.requests.clear();
    }
}

// A zero-width insertion at the edge of an edit can be ordered safely. Changes
// inside a replacement range conflict, even if both versions have equal lengths.
fn overlaps(start: usize, end: usize, other_start: usize, other_end: usize) -> bool {
    start < other_end && other_start < end
}

/// Map nonoverlapping provider edits through changes made since their snapshot.
pub(super) fn additional_edits_transaction(
    before: &Rope,
    current: &Rope,
    changes: &ChangeSet,
    edits: Vec<lsp::TextEdit>,
    encoding: lsp_client::OffsetEncoding,
) -> Option<Transaction> {
    let mut mapped = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = lsp_range_to_range(before, edit.range, encoding)?;
        let (start, end) = (range.from(), range.to());
        if changes
            .changes_iter()
            .any(|(a, b, _)| overlaps(start, end, a, b))
        {
            return None;
        }
        mapped.push((
            changes.map_pos(
                start,
                if start == end {
                    Assoc::Before
                } else {
                    Assoc::After
                },
            ),
            changes.map_pos(end, Assoc::Before),
            Some(edit.new_text.into()),
        ));
    }
    mapped.sort_unstable_by_key(|(start, end, _)| (*start, *end));
    if mapped.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return None;
    }
    Some(Transaction::change(current, mapped.into_iter()))
}

impl Editor {
    /// The menu can close or start another session while this accepted item's
    /// imports resolve. The document owns cancellation; menu lifetime is separate.
    pub(super) fn resolve_accepted_completion(&mut self, accepted: AcceptedCompletion) {
        if accepted.item.resolved
            || accepted
                .item
                .item
                .additional_text_edits
                .as_ref()
                .is_some_and(|edits| !edits.is_empty())
        {
            return;
        }
        let Some(server) = self
            .language_servers
            .get_by_id(accepted.item.provider)
            .cloned()
        else {
            return;
        };
        if !matches!(
            server.capabilities().completion_provider,
            Some(lsp::CompletionOptions {
                resolve_provider: Some(true),
                ..
            })
        ) {
            return;
        }
        let owner = Arc::downgrade(&self.handlers.completions.shared);
        let Some(doc) = self.documents.get_mut(&accepted.document) else {
            return;
        };
        let uri = doc.uri();
        let cancel = doc.accepted_completions.start();
        let callbacks = self.handlers.completions.shared.callbacks.clone();
        let future = server.resolve_completion_item(&accepted.item.item);
        tokio::spawn(async move {
            let Some(result) = event::cancelable_future(future, &cancel).await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if cancel.is_canceled()
                        || !Weak::ptr_eq(
                            &owner,
                            &Arc::downgrade(&editor.handlers.completions.shared),
                        )
                        || !editor.config().lsp.enable
                    {
                        return;
                    }
                    if !editor
                        .language_servers
                        .get_by_id(server.id())
                        .is_some_and(|current| Arc::ptr_eq(current, &server))
                    {
                        return;
                    }
                    let resolved = match result {
                        Ok(resolved) => resolved,
                        Err(error) => {
                            log::error!("Accepted completion resolve failed: {error}");
                            return;
                        }
                    };
                    let Some(edits) = resolved
                        .additional_text_edits
                        .filter(|edits| !edits.is_empty())
                    else {
                        return;
                    };
                    // Restore an unsynchronized preview before applying a real edit.
                    if editor
                        .tree
                        .try_get(editor.tree.focus)
                        .is_some_and(|view| view.doc == accepted.document)
                        && matches!(
                            editor.last_completion,
                            Some(crate::editor::CompleteAction::Selected { .. })
                        )
                    {
                        editor.restore_completion_preview();
                        editor
                            .handlers
                            .completions
                            .event(super::CompletionEvent::Cancel);
                    }
                    let Some(doc) = editor.document(accepted.document) else {
                        return;
                    };
                    if doc.uri() != uri || !doc.supports_language_server(server.id()) {
                        return;
                    }
                    let changes = accepted.savepoint.changes_since(doc.text());
                    let Some(transaction) = additional_edits_transaction(
                        accepted.before.text(),
                        doc.text(),
                        &changes,
                        edits,
                        server.offset_encoding(),
                    ) else {
                        editor.set_error(|| "Completion edits conflict with newer changes");
                        return;
                    };
                    let version = doc.version();
                    let result = if doc.get_current_revision() == accepted.before.revision() {
                        editor
                            .document_mut(accepted.document)
                            .unwrap()
                            .apply(transaction.changes())
                    } else {
                        editor.apply_document_changes(
                            accepted.document,
                            version,
                            transaction.changes(),
                        )
                    };
                    if let Err(error) = result {
                        editor.set_error(|| format!("Could not apply completion edits: {error}"));
                    }
                })
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additional_edits_map_through_typing_and_reject_overlapping_replacements() {
        let before = Rope::from("use old;\nthing\n");
        let edits = vec![lsp::TextEdit {
            range: lsp::Range::new(lsp::Position::new(0, 4), lsp::Position::new(0, 7)),
            new_text: "new".into(),
        }];
        let changes = Transaction::change(&before, [(14, 14, Some("()".into()))].into_iter());
        let mut current = before.clone();
        changes.apply(&mut current);
        let mapped = additional_edits_transaction(
            &before,
            &current,
            changes.changes(),
            edits.clone(),
            lsp_client::OffsetEncoding::Utf16,
        )
        .unwrap();
        mapped.apply(&mut current);
        assert_eq!(current, "use new;\nthing()\n");
        let conflicting = Transaction::change(&before, [(4, 7, Some("own".into()))].into_iter());
        assert!(additional_edits_transaction(
            &before,
            &before,
            conflicting.changes(),
            edits,
            lsp_client::OffsetEncoding::Utf16
        )
        .is_none());
    }
}
